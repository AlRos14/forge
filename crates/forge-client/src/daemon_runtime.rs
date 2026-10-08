use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ::time::{format_description::well_known::Rfc3339, OffsetDateTime};
use anyhow::{anyhow, Result};
use api_types::{
    DaemonErrorPayload, DaemonFrame, DaemonProtocolCapabilities, DaemonProtocolCapabilitiesRequest,
    ExecutionCancelParams, ExecutionCancelResult, ExecutionStartParams, ExecutionStartResult,
    ExecutionTerminalNotification, FsBranchesParams, FsListParams, RemoteExecutionFailureClass,
    RemoteResolvedCandidate, RemoteRouteAttempt, RemoteTokenUsage,
    DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1,
    DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1, INVALID_FRAME, METHOD_EXECUTION_CANCEL,
    METHOD_EXECUTION_LOG, METHOD_EXECUTION_START, METHOD_EXECUTION_TERMINAL, METHOD_FS_BRANCHES,
    METHOD_FS_LIST, METHOD_PROTOCOL_CAPABILITIES, METHOD_TERMINAL_INPUT, METHOD_TERMINAL_RESIZE,
    METHOD_TERMINAL_START, METHOD_TERMINAL_TERMINATE, UNSUPPORTED_METHOD,
};
use executors::{
    ExecutionContext, ExecutionFailureClass, ExecutionOutcome, ExecutionResult, ExecutorError,
    FallbackExecutor, LogEntry, TaskExecutor,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::timeout,
};

use crate::{
    daemon_fs,
    daemon_link::{run_dispatch_loop, run_with_reconnect, DaemonClient},
};

const TERMINAL_UNAVAILABLE: &str = "terminal_unavailable";
const EXECUTION_ERROR: &str = "execution_error";
const GENERATION_CANCEL_TIMEOUT: Duration = Duration::from_secs(12);
const GENERATION_TASK_TIMEOUT: Duration = Duration::from_secs(5);
const GENERATION_ABORT_TIMEOUT: Duration = Duration::from_secs(1);

/// A finished execution's terminal notification is only queued for the command
/// stream when its guard drops, so a report snapshot taken right after could
/// omit the id before the server has processed the completion — and the server
/// would reconcile the execution as daemon_disconnected. Finished ids therefore
/// stay in reports for this long after the guard drops.
const FINISHED_EXECUTION_LINGER: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ActiveExecutionTracker {
    inner: Arc<Mutex<TrackerInner>>,
    finished_linger: Duration,
}

#[derive(Default)]
struct TrackerInner {
    active: HashSet<String>,
    recently_finished: HashMap<String, Instant>,
}

impl Default for ActiveExecutionTracker {
    fn default() -> Self {
        Self::with_finished_linger(FINISHED_EXECUTION_LINGER)
    }
}

impl ActiveExecutionTracker {
    pub fn with_finished_linger(finished_linger: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TrackerInner::default())),
            finished_linger,
        }
    }

    pub fn track(&self, execution_id: String) -> ActiveExecutionGuard {
        {
            let mut inner = self.inner.lock().expect("active execution tracker lock");
            inner.recently_finished.remove(&execution_id);
            inner.active.insert(execution_id.clone());
        }
        ActiveExecutionGuard {
            tracker: self.clone(),
            execution_id,
        }
    }

    pub fn active_ids(&self) -> Vec<String> {
        let now = Instant::now();
        let linger = self.finished_linger;
        let mut inner = self.inner.lock().expect("active execution tracker lock");
        inner
            .recently_finished
            .retain(|_, finished_at| now.duration_since(*finished_at) < linger);
        let mut ids: Vec<String> = inner
            .active
            .iter()
            .chain(inner.recently_finished.keys())
            .cloned()
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

pub struct ActiveExecutionGuard {
    tracker: ActiveExecutionTracker,
    execution_id: String,
}

impl Drop for ActiveExecutionGuard {
    fn drop(&mut self) {
        let mut inner = self
            .tracker
            .inner
            .lock()
            .expect("active execution tracker lock");
        if inner.active.remove(&self.execution_id) {
            inner
                .recently_finished
                .insert(self.execution_id.clone(), Instant::now());
        }
    }
}

type CommandResult<T> = std::result::Result<T, DaemonErrorPayload>;

pub async fn run_command_stream(
    client: Arc<DaemonClient>,
    workspace_root: PathBuf,
    shutdown: watch::Receiver<bool>,
    active_executions: ActiveExecutionTracker,
) -> Result<()> {
    let workspace_root = Arc::new(workspace_root);
    run_with_reconnect(client, shutdown.clone(), move |stream| {
        let workspace_root = Arc::clone(&workspace_root);
        let shutdown = shutdown.clone();
        let active_executions = active_executions.clone();
        async move {
            let (responses_tx, responses_rx) = mpsc::unbounded_channel();
            let runtime = DaemonRuntime::new_with_tracker(
                responses_tx.clone(),
                workspace_root.as_ref().clone(),
                active_executions,
            );
            let handler = {
                let runtime = Arc::clone(&runtime);
                move |frame| {
                    let runtime = Arc::clone(&runtime);
                    async move { runtime.handle_request(frame).await }
                }
            };
            retire_generation_after_dispatch(
                &runtime,
                run_dispatch_loop(stream, handler, shutdown, responses_tx, responses_rx),
            )
            .await
        }
    })
    .await
}

/// Await one command connection's dispatch loop, then retire all executions
/// owned by that generation on both disconnect and graceful shutdown.
pub async fn retire_generation_after_dispatch<Fut>(
    runtime: &DaemonRuntime,
    dispatch: Fut,
) -> Result<()>
where
    Fut: Future<Output = Result<()>>,
{
    if let Err(error) = dispatch.await {
        tracing::warn!(%error, "daemon command dispatch ended");
    }
    runtime.retire().await
}

pub struct DaemonRuntime {
    workspace_root: PathBuf,
    outbound: mpsc::UnboundedSender<DaemonFrame>,
    executor: Arc<FallbackExecutor>,
    active_executions: ActiveExecutionTracker,
    generation: Mutex<ExecutionGeneration>,
}

#[derive(Default)]
struct ExecutionGeneration {
    retired: bool,
    owned_execution_ids: HashSet<String>,
    running_execution_ids: HashSet<String>,
    tasks: JoinSet<String>,
}

impl DaemonRuntime {
    pub fn new(outbound: mpsc::UnboundedSender<DaemonFrame>, workspace_root: PathBuf) -> Arc<Self> {
        Self::new_with_tracker(outbound, workspace_root, ActiveExecutionTracker::default())
    }

    pub fn new_with_tracker(
        outbound: mpsc::UnboundedSender<DaemonFrame>,
        workspace_root: PathBuf,
        active_executions: ActiveExecutionTracker,
    ) -> Arc<Self> {
        Self::new_with_registry_and_tracker(
            outbound,
            workspace_root,
            Arc::new(cli_adapters::default_registry()),
            active_executions,
        )
    }

    pub fn new_with_registry_and_tracker(
        outbound: mpsc::UnboundedSender<DaemonFrame>,
        workspace_root: PathBuf,
        registry: Arc<executors::HarnessAdapterRegistry>,
        active_executions: ActiveExecutionTracker,
    ) -> Arc<Self> {
        Arc::new(Self {
            workspace_root,
            outbound,
            executor: Arc::new(FallbackExecutor::new(registry)),
            active_executions,
            generation: Mutex::new(ExecutionGeneration::default()),
        })
    }

    pub fn active_execution_ids(&self) -> Vec<String> {
        self.active_executions.active_ids()
    }

    pub async fn handle_request(self: &Arc<Self>, frame: DaemonFrame) -> DaemonFrame {
        let DaemonFrame::Request { id, method, params } = frame else {
            return error_frame(
                None,
                INVALID_FRAME,
                "daemon command handler expected a request frame",
                None,
            );
        };

        match method.as_str() {
            METHOD_PROTOCOL_CAPABILITIES => {
                match decode_params::<DaemonProtocolCapabilitiesRequest>(&id, params) {
                    Ok(_) => response_frame(
                        id,
                        DaemonProtocolCapabilities {
                            schema_version: 1,
                            features: vec![
                                DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1.to_owned(),
                                DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1.to_owned(),
                            ],
                        },
                    ),
                    Err(frame) => frame,
                }
            }
            METHOD_FS_LIST => match decode_params::<FsListParams>(&id, params) {
                Ok(params) => match daemon_fs::list_entries(params, &self.workspace_root).await {
                    Ok(result) => response_frame(id, result),
                    Err(error) => DaemonFrame::Error {
                        id: Some(id),
                        error,
                    },
                },
                Err(frame) => frame,
            },
            METHOD_FS_BRANCHES => match decode_params::<FsBranchesParams>(&id, params) {
                Ok(params) => match daemon_fs::list_branches(params, &self.workspace_root).await {
                    Ok(result) => response_frame(id, result),
                    Err(error) => DaemonFrame::Error {
                        id: Some(id),
                        error,
                    },
                },
                Err(frame) => frame,
            },
            METHOD_EXECUTION_START => match decode_params::<ExecutionStartParams>(&id, params) {
                Ok(params) => match self.start(params).await {
                    Ok(result) => response_frame(id, result),
                    Err(error) => DaemonFrame::Error {
                        id: Some(id),
                        error,
                    },
                },
                Err(frame) => frame,
            },
            METHOD_EXECUTION_CANCEL => match decode_params::<ExecutionCancelParams>(&id, params) {
                Ok(params) => match self.cancel(params).await {
                    Ok(result) => response_frame(id, result),
                    Err(error) => DaemonFrame::Error {
                        id: Some(id),
                        error,
                    },
                },
                Err(frame) => frame,
            },
            METHOD_TERMINAL_START
            | METHOD_TERMINAL_INPUT
            | METHOD_TERMINAL_RESIZE
            | METHOD_TERMINAL_TERMINATE => terminal_unavailable_frame(id),
            _ => error_frame(
                Some(id),
                UNSUPPORTED_METHOD,
                format!("unsupported daemon command method: {method}"),
                None,
            ),
        }
    }

    pub async fn start(
        self: &Arc<Self>,
        params: ExecutionStartParams,
    ) -> CommandResult<ExecutionStartResult> {
        let worktree_path = daemon_fs::validate_within_root(
            Path::new(params.workspace_path.trim()),
            &self.workspace_root,
        )?;
        let logs_path = local_execution_log_path(&self.workspace_root, &params.execution_id);
        let description = prompt_description(&params.prompt);
        let ctx = ExecutionContext {
            invocation: params.invocation.clone(),
            task_id: params.task_id.clone(),
            execution_id: params.execution_id.clone(),
            role: params.role.clone(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            description,
            agent_config: params.executor_config,
            logs_path: logs_path.to_string_lossy().into_owned(),
            heartbeat_interval_seconds: 30,
            max_turns: params.max_turns,
            log_sender: None,
        };

        let execution_id = params.execution_id.clone();
        let executor = Arc::clone(&self.executor);
        let outbound = self.outbound.clone();
        let active_executions = self.active_executions.clone();
        let mut generation = self
            .generation
            .lock()
            .expect("daemon execution generation lock");
        reap_finished_generation_tasks(&mut generation);
        if generation.retired {
            return Err(execution_error(
                "daemon command connection generation is retiring",
            ));
        }
        if !generation.owned_execution_ids.insert(execution_id.clone()) {
            return Err(execution_error(format!(
                "execution {execution_id} was already admitted by this connection generation"
            )));
        }

        // Arm the cancellation state before spawning. A disconnect can retire
        // this generation as soon as Start is accepted, including before the
        // task reaches FallbackExecutor::execute.
        self.executor.prepare_execution(&execution_id);
        generation
            .running_execution_ids
            .insert(execution_id.clone());
        let task_id = execution_id.clone();
        generation.tasks.spawn(async move {
            run_execution_task(Arc::clone(&executor), outbound, ctx, active_executions).await;
            executor.finish_execution(&task_id);
            task_id
        });

        Ok(ExecutionStartResult {
            execution_id,
            accepted: true,
        })
    }

    pub async fn cancel(
        &self,
        params: ExecutionCancelParams,
    ) -> CommandResult<ExecutionCancelResult> {
        let execution_is_owned = {
            let mut generation = self
                .generation
                .lock()
                .expect("daemon execution generation lock");
            reap_finished_generation_tasks(&mut generation);
            !generation.retired
                && generation
                    .running_execution_ids
                    .contains(&params.execution_id)
        };
        if !execution_is_owned {
            return Ok(ExecutionCancelResult {
                execution_id: params.execution_id,
                cancelled: false,
            });
        }

        self.executor
            .cancel(&params.execution_id)
            .await
            .map_err(|error| execution_error(format!("failed to cancel execution: {error}")))?;
        Ok(ExecutionCancelResult {
            execution_id: params.execution_id,
            cancelled: true,
        })
    }

    /// Retire this command connection generation and stop every Execution it
    /// launched through the exact executor and adapter instances that own its
    /// live process handles. Reconnected runtimes never adopt these tasks.
    pub async fn retire(&self) -> Result<()> {
        let (mut tasks, execution_ids) = {
            let mut generation = self
                .generation
                .lock()
                .expect("daemon execution generation lock");
            generation.retired = true;
            reap_finished_generation_tasks(&mut generation);
            generation.running_execution_ids.clear();
            // Include executions whose Rust tasks already returned. An adapter
            // can retain a child handle on an early error path, and only that
            // generation's executor still knows how to terminate it.
            let execution_ids = std::mem::take(&mut generation.owned_execution_ids)
                .into_iter()
                .collect::<Vec<_>>();
            (std::mem::take(&mut generation.tasks), execution_ids)
        };

        let executor = Arc::clone(&self.executor);
        if let Err(error) = timeout(
            GENERATION_CANCEL_TIMEOUT,
            cancel_generation_executions(&executor, &execution_ids),
        )
        .await
        .map_err(|_| {
            anyhow!("timed out cancelling executions while retiring daemon connection generation")
        })
        .and_then(|result| result)
        {
            tracing::warn!(
                execution_count = execution_ids.len(),
                error = %error,
                "initial daemon generation cancellation did not complete cleanly"
            );
        }

        let joined = timeout(GENERATION_TASK_TIMEOUT, join_generation_tasks(&mut tasks)).await;
        let task_join_failures = match joined {
            Ok(failures) => failures,
            Err(_) => {
                tracing::warn!(
                execution_count = execution_ids.len(),
                timeout_secs = GENERATION_TASK_TIMEOUT.as_secs(),
                "execution tasks did not finish after daemon generation cancellation; aborting remaining tasks"
            );
                tasks.abort_all();
                let aborted =
                    timeout(GENERATION_ABORT_TIMEOUT, join_generation_tasks(&mut tasks)).await;

                // Cancellation state is execution-task bookkeeping, not evidence
                // that a child process is alive. Release it after the old tasks
                // have been aborted; the executor instance is never reused.
                for execution_id in &execution_ids {
                    self.executor.finish_execution(execution_id);
                }

                let post_abort_cancel = timeout(
                    GENERATION_CANCEL_TIMEOUT,
                    cancel_generation_executions(&executor, &execution_ids),
                )
                .await;
                let aborted_task_failures = match aborted {
                    Ok(failures) => failures,
                    Err(_) => vec!["aborted execution tasks did not join".to_owned()],
                };
                let post_abort_cancel_failure = match post_abort_cancel {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("post-abort process termination timed out".to_owned()),
                };
                let mut reasons = aborted_task_failures;
                if let Some(error) = post_abort_cancel_failure {
                    reasons.push(error);
                }
                reasons.push(
                    "an aborted execution task cannot prove that its child process terminated"
                        .to_owned(),
                );
                return Err(anyhow!(
                    "daemon generation teardown failed closed: {}",
                    reasons.join("; ")
                ));
            }
        };

        let post_join_cancel = timeout(
            GENERATION_CANCEL_TIMEOUT,
            cancel_generation_executions(&executor, &execution_ids),
        )
        .await;
        for execution_id in &execution_ids {
            self.executor.finish_execution(execution_id);
        }

        match post_join_cancel {
            Ok(Ok(())) if task_join_failures.is_empty() => Ok(()),
            Ok(Ok(())) => Err(anyhow!(
                "daemon generation execution tasks failed while joining: {}",
                task_join_failures.join(", ")
            )),
            Ok(Err(error)) => Err(error.context(
                "failed to verify process termination after daemon generation tasks joined",
            )),
            Err(_) => Err(anyhow!(
                "timed out verifying process termination after daemon generation tasks joined"
            )),
        }
    }
}

fn reap_finished_generation_tasks(generation: &mut ExecutionGeneration) {
    while let Some(result) = generation.tasks.try_join_next() {
        if let Ok(execution_id) = result {
            generation.running_execution_ids.remove(&execution_id);
        }
    }
}

async fn cancel_generation_executions(
    executor: &FallbackExecutor,
    execution_ids: &[String],
) -> Result<()> {
    let results =
        futures_util::future::join_all(execution_ids.iter().map(|execution_id| async move {
            (execution_id, executor.cancel(execution_id).await)
        }))
        .await;
    let failures = results
        .into_iter()
        .filter_map(|(execution_id, result)| {
            result.err().map(|error| format!("{execution_id}: {error}"))
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "adapter cancellation failed: {}",
            failures.join(", ")
        ))
    }
}

async fn join_generation_tasks(tasks: &mut JoinSet<String>) -> Vec<String> {
    let mut failures = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!(%error, "daemon execution task failed while joining its generation");
            failures.push(error.to_string());
        }
    }
    failures
}

async fn run_execution_task(
    executor: Arc<FallbackExecutor>,
    outbound: mpsc::UnboundedSender<DaemonFrame>,
    mut ctx: ExecutionContext,
    active_executions: ActiveExecutionTracker,
) {
    let _active_guard = active_executions.track(ctx.execution_id.clone());
    let (log_tx, mut log_rx) = mpsc::unbounded_channel::<LogEntry>();
    ctx.log_sender = Some(log_tx);
    let log_outbound = outbound.clone();
    let mut log_forwarder = tokio::spawn(async move {
        while let Some(entry) = log_rx.recv().await {
            emit_execution_log(&log_outbound, entry);
        }
    });

    emit_execution_log(
        &outbound,
        daemon_system_log(&ctx.execution_id, "remote daemon execution started"),
    );

    let execution_id = ctx.execution_id.clone();
    let read_only_path = executors::is_worktree_read_only(&ctx.agent_config)
        .then(|| PathBuf::from(&ctx.worktree_path));
    let read_only_head = match read_only_path.as_deref() {
        Some(path) => git::get_current_sha(path).await.map(Some).map_err(|error| {
            ExecutorError::Other(format!(
                "failed to capture read-only worktree state: {error}"
            ))
        }),
        None => Ok(None),
    };
    let result = match read_only_head {
        Ok(read_only_head) => {
            let logs_path = PathBuf::from(&ctx.logs_path);
            let execution_result = executor.execute(ctx).await;
            if let Err(error) = executors::LogWriter::compact(&logs_path).await {
                tracing::warn!(%error, "failed to compress daemon execution log; plain log retained");
            }
            let restore_result = match (read_only_path.as_deref(), read_only_head.as_deref()) {
                (Some(path), Some(head)) => {
                    git::restore_worktree(path, head).await.map_err(|error| {
                        ExecutorError::Other(format!(
                            "failed to restore read-only worktree state: {error}"
                        ))
                    })
                }
                _ => Ok(()),
            };
            match (execution_result, restore_result) {
                (_, Err(error)) => Err(error),
                (Ok(mut result), Ok(())) => {
                    if let Some(head) = read_only_head {
                        result.after_sha = Some(head);
                    }
                    Ok(result)
                }
                (Err(error), Ok(())) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    // The executor owns the only log sender in ctx, so completion should close the
    // channel and let the forwarder drain. If an executor holds a sender clone or
    // emits a very large trailing burst, the timeout favors terminal notification
    // over complete best-effort log delivery.
    if tokio::time::timeout(Duration::from_secs(2), &mut log_forwarder)
        .await
        .is_err()
    {
        log_forwarder.abort();
        let _ = log_forwarder.await;
    }

    let notification = match result {
        Ok(result) => terminal_notification_from_result(execution_id, result),
        Err(error) => ExecutionTerminalNotification {
            execution_id,
            exit_code: Some(1),
            signal: None,
            error: Some(error.to_string()),
            ts: rfc3339_now(),
            status: Some("failed".to_owned()),
            agent_session_id: None,
            summary: None,
            assistant_output: None,
            after_sha: None,
            usage: None,
            account_usage: None,
            failure_class: None,
            retry_at: None,
            resolved_candidate: None,
            route_attempts: None,
        },
    };
    emit_notification(&outbound, METHOD_EXECUTION_TERMINAL, notification);
}

fn terminal_notification_from_result(
    execution_id: String,
    result: ExecutionResult,
) -> ExecutionTerminalNotification {
    let (status, exit_code, signal, error) = match result.status {
        ExecutionOutcome::Completed => ("completed", Some(0), None, None),
        ExecutionOutcome::Failed => ("failed", Some(1), None, result.error),
        ExecutionOutcome::Cancelled => ("cancelled", None, Some("cancelled".to_owned()), None),
    };
    ExecutionTerminalNotification {
        execution_id,
        exit_code,
        signal,
        error,
        ts: rfc3339_now(),
        status: Some(status.to_owned()),
        agent_session_id: result.agent_session_id,
        summary: result.summary,
        assistant_output: result.assistant_output,
        after_sha: result.after_sha,
        usage: result.usage.map(|usage| RemoteTokenUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            cost_usd: usage.cost_usd,
            model: usage.model,
        }),
        account_usage: result.account_usage,
        failure_class: result.failure_class.map(|class| match class {
            ExecutionFailureClass::TaskFailed => RemoteExecutionFailureClass::TaskFailed,
            ExecutionFailureClass::ExecutorUnavailable => {
                RemoteExecutionFailureClass::ExecutorUnavailable
            }
        }),
        retry_at: result.retry_after.and_then(|retry_after| {
            (OffsetDateTime::now_utc() + retry_after)
                .format(&Rfc3339)
                .ok()
        }),
        resolved_candidate: result
            .resolved_candidate
            .map(|candidate| RemoteResolvedCandidate {
                candidate_key: candidate.candidate_key,
                executor_type: candidate.executor_type.to_string(),
                config: candidate.config,
                harness_capabilities: Some(candidate.harness_capabilities.snapshot()),
                effective_policy: Some(candidate.effective_policy),
            }),
        route_attempts: if result.route_attempts.is_empty() {
            None
        } else {
            Some(
                result
                    .route_attempts
                    .into_iter()
                    .map(|attempt| RemoteRouteAttempt {
                        candidate_key: attempt.candidate_key,
                        outcome: attempt.outcome.as_str().to_owned(),
                    })
                    .collect(),
            )
        },
    }
}

fn daemon_system_log(execution_id: &str, line: &str) -> LogEntry {
    LogEntry {
        schema_version: 1,
        sequence: 0,
        timestamp: rfc3339_now(),
        execution_id: execution_id.to_owned(),
        kind: executors::LogKind::System,
        stream: executors::LogStream::Main,
        payload: serde_json::json!({ "line": line }),
        truncated: false,
    }
}

fn emit_execution_log(outbound: &mpsc::UnboundedSender<DaemonFrame>, entry: LogEntry) {
    let line = entry
        .payload
        .get("line")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| entry.payload.to_string());
    let stream = match entry.kind {
        executors::LogKind::Stderr => "stderr",
        _ => "stdout",
    };
    let notification = api_types::ExecutionLogNotification {
        execution_id: entry.execution_id.clone(),
        seq: entry.sequence,
        stream: stream.to_owned(),
        line,
        ts: entry.timestamp.clone(),
        kind: Some(entry.kind.to_string()),
        log_stream: Some(
            match entry.stream {
                executors::LogStream::Heartbeat => "heartbeat",
                executors::LogStream::Main => "main",
            }
            .to_owned(),
        ),
        payload: Some(entry.payload),
        truncated: Some(entry.truncated),
    };
    emit_notification(outbound, METHOD_EXECUTION_LOG, notification);
}

fn emit_notification<T: Serialize>(
    outbound: &mpsc::UnboundedSender<DaemonFrame>,
    method: &str,
    notification: T,
) {
    match serde_json::to_value(notification) {
        Ok(params) => {
            let _ = outbound.send(DaemonFrame::Notification {
                method: method.to_owned(),
                params,
            });
        }
        Err(error) => {
            tracing::warn!(%error, method, "failed to serialize daemon notification");
        }
    }
}

fn terminal_unavailable_frame(id: String) -> DaemonFrame {
    error_frame(
        Some(id),
        TERMINAL_UNAVAILABLE,
        "terminal support is not available in this daemon command context",
        None,
    )
}

fn local_execution_log_path(workspace_root: &Path, execution_id: &str) -> PathBuf {
    workspace_root
        .join(".forge-daemon")
        .join("execution-logs")
        .join(format!("{}.jsonl", safe_path_component(execution_id)))
}

fn safe_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn prompt_description(prompt: &Value) -> String {
    prompt
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| prompt.to_string())
}

fn execution_error(message: impl Into<String>) -> DaemonErrorPayload {
    DaemonErrorPayload {
        code: EXECUTION_ERROR.to_owned(),
        message: message.into(),
        details: None,
    }
}

fn decode_params<T: DeserializeOwned>(
    id: &str,
    params: serde_json::Value,
) -> std::result::Result<T, DaemonFrame> {
    serde_json::from_value(params).map_err(|error| {
        error_frame(
            Some(id.to_owned()),
            INVALID_FRAME,
            format!("invalid daemon command params: {error}"),
            None,
        )
    })
}

fn response_frame<T: Serialize>(id: String, result: T) -> DaemonFrame {
    match serde_json::to_value(result) {
        Ok(result) => DaemonFrame::Response { id, result },
        Err(error) => error_frame(
            Some(id),
            INVALID_FRAME,
            format!("failed to serialize daemon command result: {error}"),
            None,
        ),
    }
}

fn error_frame(
    id: Option<String>,
    code: impl Into<String>,
    message: impl Into<String>,
    details: Option<serde_json::Value>,
) -> DaemonFrame {
    DaemonFrame::Error {
        id,
        error: DaemonErrorPayload {
            code: code.into(),
            message: message.into(),
            details,
        },
    }
}

fn rfc3339_now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use super::*;
    use api_types::{
        ExecutionLogNotification, FsListResult,
        DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1, METHOD_EXECUTION_LOG,
        METHOD_EXECUTION_TERMINAL, METHOD_FS_LIST, METHOD_PROTOCOL_CAPABILITIES,
    };
    use async_trait::async_trait;
    use executors::{
        AvailabilityInfo, AvailabilityStatus, DiscoverContext, DiscoveredOptions, ExecutionContext,
        ExecutionOutcome, ExecutionResult, ExecutorError, ExecutorKind, HarnessAdapter,
        HarnessAdapterRegistry, ProcessGroupChild,
    };
    use tokio::sync::{mpsc, Notify};
    use tokio::{io::AsyncReadExt, process::Command};
    use tokio_util::sync::CancellationToken;

    struct ResumeRecordingAdapter {
        invocations: Arc<Mutex<Vec<executors::HarnessInvocation>>>,
    }

    #[async_trait]
    impl HarnessAdapter for ResumeRecordingAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Codex
        }

        fn check_availability(&self) -> AvailabilityInfo {
            AvailabilityInfo {
                status: AvailabilityStatus::Authenticated,
                authenticated_at: None,
                config_path: None,
            }
        }

        fn capabilities(&self, _config: &serde_json::Value) -> api_types::HarnessCapabilities {
            let mut capabilities = api_types::HarnessCapabilities::unsupported();
            capabilities.resume = api_types::CapabilitySupport::Native;
            capabilities.cancel = api_types::CapabilitySupport::Emulated;
            capabilities
        }

        async fn discover_options(
            &self,
            _ctx: DiscoverContext,
        ) -> Result<DiscoveredOptions, ExecutorError> {
            Ok(DiscoveredOptions::default())
        }

        async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            self.invocations.lock().unwrap().push(ctx.invocation);
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                ..Default::default()
            })
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    struct CursorExecutionUsageAdapter {
        executions: Arc<AtomicUsize>,
        usage_observations: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl HarnessAdapter for CursorExecutionUsageAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Cursor
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

        async fn execute(&self, _ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                ..Default::default()
            })
        }

        async fn observe_usage(
            &self,
            _config: &serde_json::Value,
            _cancel: CancellationToken,
        ) -> Result<Option<executors::UsageObservation>, ExecutorError> {
            self.usage_observations.fetch_add(1, Ordering::SeqCst);
            Ok(Some(executors::UsageObservation {
                value: serde_json::json!({"plan": "fixture"}),
                source: Some("cursor_poll".to_owned()),
            }))
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    struct ControlledExecution {
        started: AtomicBool,
        running: AtomicBool,
        cancellations: AtomicUsize,
        started_notify: Notify,
        cancel: CancellationToken,
        complete: CancellationToken,
    }

    impl ControlledExecution {
        fn new() -> Self {
            Self {
                started: AtomicBool::new(false),
                running: AtomicBool::new(false),
                cancellations: AtomicUsize::new(0),
                started_notify: Notify::new(),
                cancel: CancellationToken::new(),
                complete: CancellationToken::new(),
            }
        }
    }

    struct ControlledAdapter {
        execution: Arc<ControlledExecution>,
    }

    #[async_trait]
    impl HarnessAdapter for ControlledAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Codex
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

        async fn execute(&self, _ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            self.execution.running.store(true, Ordering::SeqCst);
            self.execution.started.store(true, Ordering::SeqCst);
            self.execution.started_notify.notify_one();
            let status = tokio::select! {
                () = self.execution.cancel.cancelled() => ExecutionOutcome::Cancelled,
                () = self.execution.complete.cancelled() => ExecutionOutcome::Completed,
            };
            self.execution.running.store(false, Ordering::SeqCst);
            Ok(ExecutionResult {
                status,
                ..Default::default()
            })
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            self.execution.cancellations.fetch_add(1, Ordering::SeqCst);
            self.execution.cancel.cancel();
            Ok(())
        }
    }

    struct ChildProcessState {
        child: Mutex<Option<Arc<tokio::sync::Mutex<ProcessGroupChild>>>>,
        pid: AtomicU32,
        descendant_pid: AtomicU32,
        descendant_pid_file: PathBuf,
        spawned: Notify,
        hold_before_registration: bool,
    }

    struct ChildProcessAdapter {
        state: Arc<ChildProcessState>,
    }

    #[async_trait]
    impl HarnessAdapter for ChildProcessAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Codex
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

        async fn execute(&self, _ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            let mut command = Command::new("sh");
            command
                .args([
                    "-c",
                    &format!(
                        "sleep 60 & echo $! > '{}' ; exec sleep 60",
                        self.state.descendant_pid_file.display()
                    ),
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            let mut child = ProcessGroupChild::spawn(&mut command)?;
            self.state
                .pid
                .store(child.id().expect("leader has pid"), Ordering::SeqCst);

            let descendant_pid = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(value) =
                        tokio::fs::read_to_string(&self.state.descendant_pid_file).await
                    {
                        if let Ok(pid) = value.trim().parse::<u32>() {
                            break pid;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| ExecutorError::Other("descendant did not write its PID".to_owned()))?;
            self.state
                .descendant_pid
                .store(descendant_pid, Ordering::SeqCst);

            if self.state.hold_before_registration {
                self.state.spawned.notify_one();
                std::future::pending::<()>().await;
            }

            let stdout = child.inner().stdout.take().expect("child stdout is piped");
            let child = Arc::new(tokio::sync::Mutex::new(child));
            *self.state.child.lock().expect("child process lock") = Some(child.clone());
            self.state.spawned.notify_one();

            let mut output = Vec::new();
            let mut stdout = stdout;
            stdout.read_to_end(&mut output).await?;
            let status = child.lock().await.kill_and_wait().await?;
            self.state.child.lock().expect("child process lock").take();
            Ok(ExecutionResult {
                status: if status.success() {
                    ExecutionOutcome::Completed
                } else {
                    ExecutionOutcome::Failed
                },
                ..Default::default()
            })
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            let child = self.state.child.lock().expect("child process lock").clone();
            if let Some(child) = child {
                let mut child = child.lock().await;
                child.kill_and_wait().await?;
            }
            Ok(())
        }
    }

    fn child_process_runtime(
        workspace_root: &Path,
        state: Arc<ChildProcessState>,
        outbound: mpsc::UnboundedSender<DaemonFrame>,
    ) -> Arc<DaemonRuntime> {
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(ChildProcessAdapter { state }));
        DaemonRuntime::new_with_registry_and_tracker(
            outbound,
            workspace_root.to_path_buf(),
            Arc::new(registry),
            ActiveExecutionTracker::with_finished_linger(Duration::ZERO),
        )
    }

    fn controlled_runtime(
        workspace_root: &Path,
        execution: Arc<ControlledExecution>,
        tracker: ActiveExecutionTracker,
        outbound: mpsc::UnboundedSender<DaemonFrame>,
    ) -> Arc<DaemonRuntime> {
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(ControlledAdapter { execution }));
        DaemonRuntime::new_with_registry_and_tracker(
            outbound,
            workspace_root.to_path_buf(),
            Arc::new(registry),
            tracker,
        )
    }

    fn controlled_start_params(workspace_root: &Path, execution_id: &str) -> ExecutionStartParams {
        ExecutionStartParams {
            task_id: "task-generation".to_owned(),
            execution_id: execution_id.to_owned(),
            role: "coder".to_owned(),
            workspace_path: workspace_root.to_string_lossy().into_owned(),
            executor_type: "codex".to_owned(),
            executor_config: serde_json::json!({
                "executor_type": "codex",
                "config": {}
            }),
            prompt: serde_json::json!({ "description": "controlled execution" }),
            invocation: executors::HarnessInvocation::Start,
            max_turns: None,
        }
    }

    async fn wait_until_started(execution: &ControlledExecution) {
        if execution.started.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::timeout(Duration::from_secs(2), execution.started_notify.notified())
            .await
            .expect("controlled execution starts");
        assert!(execution.started.load(Ordering::SeqCst));
    }

    #[test]
    fn terminal_notification_transmits_full_assistant_output_separately_from_summary() {
        let notification = terminal_notification_from_result(
            "execution-plan".to_owned(),
            ExecutionResult {
                status: ExecutionOutcome::Completed,
                assistant_output: Some("# Full plan\nDetailed steps\n".to_owned()),
                summary: Some("short summary".to_owned()),
                ..Default::default()
            },
        );

        assert_eq!(
            notification.assistant_output.as_deref(),
            Some("# Full plan\nDetailed steps\n")
        );
        assert_eq!(notification.summary.as_deref(), Some("short summary"));
    }

    #[tokio::test]
    async fn fs_list_returns_entries_under_workspace_root() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        fs::create_dir_all(dir.path().join("src")).expect("src creates");
        fs::write(dir.path().join("README.md"), "readme").expect("readme writes");
        let (tx, _rx) = mpsc::unbounded_channel();
        let runtime = DaemonRuntime::new(tx, dir.path().to_path_buf());

        let frame = DaemonFrame::Request {
            id: "fs-1".to_owned(),
            method: METHOD_FS_LIST.to_owned(),
            params: serde_json::json!({ "path": "." }),
        };
        let response = runtime.handle_request(frame).await;

        let DaemonFrame::Response { result, .. } = response else {
            panic!("expected response");
        };
        let result: FsListResult = serde_json::from_value(result).expect("fs result parses");
        let names = result
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["src", "README.md"]);
    }

    #[tokio::test]
    async fn daemon_advertises_generic_harness_invocation_protocol() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, _rx) = mpsc::unbounded_channel();
        let runtime = DaemonRuntime::new(tx, dir.path().to_path_buf());
        let response = runtime
            .handle_request(DaemonFrame::Request {
                id: "protocol-1".to_owned(),
                method: METHOD_PROTOCOL_CAPABILITIES.to_owned(),
                params: serde_json::json!({}),
            })
            .await;
        let DaemonFrame::Response { result, .. } = response else {
            panic!("expected protocol capability response");
        };
        let capabilities: DaemonProtocolCapabilities =
            serde_json::from_value(result).expect("protocol response parses");
        assert!(capabilities.supports(DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1));
        assert!(capabilities.supports(DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1));
    }

    #[tokio::test]
    async fn pre_pr3_resume_request_is_rejected_before_adapter_dispatch() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, _rx) = mpsc::unbounded_channel();
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(ResumeRecordingAdapter {
            invocations: Arc::clone(&invocations),
        }));
        let runtime = DaemonRuntime::new_with_registry_and_tracker(
            tx,
            dir.path().to_path_buf(),
            Arc::new(registry),
            ActiveExecutionTracker::default(),
        );

        let response = runtime
            .handle_request(DaemonFrame::Request {
                id: "legacy-resume".to_owned(),
                method: METHOD_EXECUTION_START.to_owned(),
                params: serde_json::json!({
                    "task_id": "task-legacy",
                    "execution_id": "exec-legacy",
                    "workspace_path": dir.path().to_string_lossy(),
                    "executor_type": "codex",
                    "executor_config": { "resume_thread_id": "session-123" },
                    "prompt": { "description": "continue" },
                    "max_turns": null
                }),
            })
            .await;

        let DaemonFrame::Error { error, .. } = response else {
            panic!("legacy request without invocation must be rejected");
        };
        assert_eq!(error.code, api_types::INVALID_FRAME);
        assert!(error.message.contains("invocation"));
        assert!(invocations.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn shell_execution_reports_completion_notification() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = DaemonRuntime::new(tx, dir.path().to_path_buf());
        let execution_id = "exec-shell-ok".to_owned();

        let result = runtime
            .start(ExecutionStartParams {
                task_id: "task-1".to_owned(),
                execution_id: execution_id.clone(),
                role: "coder".to_owned(),
                workspace_path: dir.path().to_string_lossy().into_owned(),
                executor_type: "shell".to_owned(),
                executor_config: serde_json::json!({
                    "executor_type": "shell",
                    "config": {}
                }),
                prompt: serde_json::json!({ "description": "printf ok > marker.txt" }),
                invocation: executors::HarnessInvocation::Start,
                max_turns: None,
            })
            .await
            .expect("execution starts");
        assert!(result.accepted);

        let notification = next_terminal_notification(&mut rx, &execution_id).await;
        assert_eq!(notification.status.as_deref(), Some("completed"));
        assert_eq!(
            fs::read_to_string(dir.path().join("marker.txt")).expect("marker exists"),
            "ok"
        );
    }

    #[tokio::test]
    async fn remote_resume_intent_reaches_harness_adapter_and_reports_capabilities() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(ResumeRecordingAdapter {
            invocations: Arc::clone(&invocations),
        }));
        let runtime = DaemonRuntime::new_with_registry_and_tracker(
            tx,
            dir.path().to_path_buf(),
            Arc::new(registry),
            ActiveExecutionTracker::default(),
        );
        let invocation = api_types::HarnessInvocation::Resume {
            external_session_id: "remote-session-abc".to_owned(),
        };

        runtime
            .start(ExecutionStartParams {
                task_id: "task-remote-resume".to_owned(),
                execution_id: "exec-remote-resume".to_owned(),
                role: "coder".to_owned(),
                workspace_path: dir.path().to_string_lossy().into_owned(),
                executor_type: "codex".to_owned(),
                executor_config: serde_json::json!({
                    "executor_type":"codex",
                    "config":{"model":"test"}
                }),
                prompt: serde_json::json!({"description":"resume this exact session"}),
                invocation: invocation.clone(),
                max_turns: None,
            })
            .await
            .expect("remote execution is accepted");

        let terminal = next_terminal_notification(&mut rx, "exec-remote-resume").await;
        assert_eq!(*invocations.lock().unwrap(), vec![invocation]);
        let winner = terminal
            .resolved_candidate
            .expect("remote result includes the selected adapter");
        let capabilities = winner
            .harness_capabilities
            .expect("remote winner carries versioned capability evidence");
        let api_types::HarnessCapabilitiesSnapshotRead::VersionedV1(capabilities) =
            api_types::HarnessCapabilitiesSnapshot::from_value(
                &serde_json::to_value(capabilities).unwrap(),
            )
        else {
            panic!("winner capability evidence is readable");
        };
        assert_eq!(capabilities.resume, api_types::CapabilitySupport::Native);
        assert_eq!(capabilities.cancel, api_types::CapabilitySupport::Emulated);
    }

    #[tokio::test]
    async fn cursor_execution_completes_without_periodic_usage_observation() {
        let dir = tempfile::tempdir().expect("workspace root creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let executions = Arc::new(AtomicUsize::new(0));
        let usage_observations = Arc::new(AtomicUsize::new(0));
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(CursorExecutionUsageAdapter {
            executions: Arc::clone(&executions),
            usage_observations: Arc::clone(&usage_observations),
        }));
        let runtime = DaemonRuntime::new_with_registry_and_tracker(
            tx,
            dir.path().to_path_buf(),
            Arc::new(registry),
            ActiveExecutionTracker::default(),
        );

        runtime
            .start(ExecutionStartParams {
                task_id: "task-cursor-usage".to_owned(),
                execution_id: "exec-cursor-usage".to_owned(),
                role: "coder".to_owned(),
                workspace_path: dir.path().to_string_lossy().into_owned(),
                executor_type: "cursor".to_owned(),
                executor_config: serde_json::json!({
                    "executor_type": "cursor",
                    "config": {}
                }),
                prompt: serde_json::json!({"description": "complete Cursor execution"}),
                invocation: executors::HarnessInvocation::Start,
                max_turns: None,
            })
            .await
            .expect("Cursor execution is accepted");

        let terminal = next_terminal_notification(&mut rx, "exec-cursor-usage").await;

        assert_eq!(terminal.status.as_deref(), Some("completed"));
        assert!(
            terminal.account_usage.is_none(),
            "no current Cursor observation is reported when polling is disabled"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(
            usage_observations.load(Ordering::SeqCst),
            0,
            "Execution must not launch a Cursor usage helper"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_execution_can_be_cancelled() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = DaemonRuntime::new(tx, dir.path().to_path_buf());
        let execution_id = "exec-shell-cancel".to_owned();

        runtime
            .start(ExecutionStartParams {
                task_id: "task-1".to_owned(),
                execution_id: execution_id.clone(),
                role: "coder".to_owned(),
                workspace_path: dir.path().to_string_lossy().into_owned(),
                executor_type: "shell".to_owned(),
                executor_config: serde_json::json!({
                    "executor_type": "shell",
                    "config": {}
                }),
                prompt: serde_json::json!({ "description": "printf 'started\\n'; sleep 30" }),
                invocation: executors::HarnessInvocation::Start,
                max_turns: None,
            })
            .await
            .expect("execution starts");
        next_execution_log_line(&mut rx, &execution_id, "started").await;
        runtime
            .cancel(ExecutionCancelParams {
                execution_id: execution_id.clone(),
                reason: Some("test".to_owned()),
            })
            .await
            .expect("execution cancels");

        let notification = next_terminal_notification(&mut rx, &execution_id).await;
        assert_eq!(notification.status.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn lost_connection_generation_cancels_and_joins_its_running_execution() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = Arc::new(ControlledExecution::new());
        let tracker = ActiveExecutionTracker::with_finished_linger(Duration::ZERO);
        let runtime = controlled_runtime(dir.path(), Arc::clone(&execution), tracker.clone(), tx);

        runtime
            .start(controlled_start_params(dir.path(), "exec-generation-e"))
            .await
            .expect("execution starts on generation A1");
        wait_until_started(&execution).await;
        assert!(execution.running.load(Ordering::SeqCst));
        assert_eq!(runtime.active_execution_ids(), ["exec-generation-e"]);

        retire_generation_after_dispatch(&runtime, async {
            Err(anyhow::anyhow!("command socket dropped"))
        })
        .await
        .expect("generation retires its execution after disconnect");

        assert_eq!(execution.cancellations.load(Ordering::SeqCst), 2);
        assert!(!execution.running.load(Ordering::SeqCst));
        assert!(runtime.active_execution_ids().is_empty());
        assert!(tracker.active_ids().is_empty());
        let terminal = next_terminal_notification(&mut rx, "exec-generation-e").await;
        assert_eq!(terminal.status.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn disconnect_cancels_start_accepted_before_its_adapter_begins() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = Arc::new(ControlledExecution::new());
        let tracker = ActiveExecutionTracker::with_finished_linger(Duration::ZERO);
        let runtime = controlled_runtime(dir.path(), Arc::clone(&execution), tracker.clone(), tx);

        runtime
            .start(controlled_start_params(dir.path(), "exec-pending-start"))
            .await
            .expect("Start is admitted before disconnect");
        retire_generation_after_dispatch(&runtime, async {
            Err(anyhow::anyhow!("connection dropped before adapter start"))
        })
        .await
        .expect("pending Start is retired with its generation");

        assert_eq!(execution.cancellations.load(Ordering::SeqCst), 2);
        assert!(!execution.running.load(Ordering::SeqCst));
        assert!(tracker.active_ids().is_empty());
        let terminal = next_terminal_notification(&mut rx, "exec-pending-start").await;
        assert_eq!(terminal.status.as_deref(), Some("cancelled"));
    }

    #[tokio::test]
    async fn new_connection_generation_does_not_adopt_old_execution_and_runs_new_work() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let tracker = ActiveExecutionTracker::with_finished_linger(Duration::ZERO);
        let (a1_tx, mut a1_rx) = mpsc::unbounded_channel();
        let a1_execution = Arc::new(ControlledExecution::new());
        let a1 = controlled_runtime(
            dir.path(),
            Arc::clone(&a1_execution),
            tracker.clone(),
            a1_tx,
        );
        a1.start(controlled_start_params(dir.path(), "exec-old-generation"))
            .await
            .expect("A1 starts E");
        wait_until_started(&a1_execution).await;
        retire_generation_after_dispatch(&a1, async { Err(anyhow::anyhow!("A1 disconnected")) })
            .await
            .expect("A1 terminates E before retirement");
        let terminal = next_terminal_notification(&mut a1_rx, "exec-old-generation").await;
        assert_eq!(terminal.status.as_deref(), Some("cancelled"));
        assert!(!a1_execution.running.load(Ordering::SeqCst));

        // The logical daemon identity is unchanged across this replacement;
        // only its in-memory command generation and runtime are new.
        let (a2_tx, mut a2_rx) = mpsc::unbounded_channel();
        let a2_execution = Arc::new(ControlledExecution::new());
        let a2 = controlled_runtime(
            dir.path(),
            Arc::clone(&a2_execution),
            tracker.clone(),
            a2_tx,
        );
        let old_cancel = a2
            .cancel(ExecutionCancelParams {
                execution_id: "exec-old-generation".to_owned(),
                reason: Some("recovery".to_owned()),
            })
            .await
            .expect("A2 rejects control over E from A1");
        assert!(!old_cancel.cancelled);
        assert_eq!(a2_execution.cancellations.load(Ordering::SeqCst), 0);

        a2.start(controlled_start_params(dir.path(), "exec-new-generation"))
            .await
            .expect("A2 starts F normally");
        wait_until_started(&a2_execution).await;
        a2_execution.complete.cancel();
        let terminal = next_terminal_notification(&mut a2_rx, "exec-new-generation").await;
        assert_eq!(terminal.status.as_deref(), Some("completed"));
        assert!(!a2_execution.running.load(Ordering::SeqCst));
        a2.retire().await.expect("A2 retires cleanly");
    }

    #[tokio::test]
    async fn graceful_daemon_shutdown_retires_running_generation_execution() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execution = Arc::new(ControlledExecution::new());
        let tracker = ActiveExecutionTracker::with_finished_linger(Duration::ZERO);
        let runtime = controlled_runtime(dir.path(), Arc::clone(&execution), tracker.clone(), tx);

        runtime
            .start(controlled_start_params(dir.path(), "exec-daemon-shutdown"))
            .await
            .expect("execution starts before shutdown");
        wait_until_started(&execution).await;
        retire_generation_after_dispatch(&runtime, async { Ok(()) })
            .await
            .expect("graceful dispatch exit retires its execution");

        assert_eq!(execution.cancellations.load(Ordering::SeqCst), 2);
        assert!(!execution.running.load(Ordering::SeqCst));
        assert!(tracker.active_ids().is_empty());
        let terminal = next_terminal_notification(&mut rx, "exec-daemon-shutdown").await;
        assert_eq!(terminal.status.as_deref(), Some("cancelled"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_generation_retirement_proves_real_process_tree_dead_before_a2() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, _rx) = mpsc::unbounded_channel();
        let state = Arc::new(ChildProcessState {
            child: Mutex::new(None),
            pid: AtomicU32::new(0),
            descendant_pid: AtomicU32::new(0),
            descendant_pid_file: dir.path().join("a1-descendant.pid"),
            spawned: Notify::new(),
            hold_before_registration: false,
        });
        let runtime = child_process_runtime(dir.path(), Arc::clone(&state), tx);

        runtime
            .start(controlled_start_params(dir.path(), "exec-real-child"))
            .await
            .expect("execution starts on generation A1");
        tokio::time::timeout(Duration::from_secs(2), state.spawned.notified())
            .await
            .expect("real child starts");
        let pid = state.pid.load(Ordering::SeqCst);
        let descendant_pid = state.descendant_pid.load(Ordering::SeqCst);
        assert_ne!(pid, 0);
        assert_ne!(descendant_pid, 0);
        assert!(
            process_is_executable(pid),
            "A1 leader is live before retirement"
        );
        assert!(
            process_is_executable(descendant_pid),
            "A1 descendant is live before retirement"
        );

        runtime
            .retire()
            .await
            .expect("successful retirement proves process-tree termination");
        assert!(
            !process_is_executable(pid),
            "A1 leader is dead on retirement"
        );
        assert!(
            !process_is_executable(descendant_pid),
            "A1 descendant is dead on retirement"
        );

        let (a2_tx, _a2_rx) = mpsc::unbounded_channel();
        let a2_state = Arc::new(ChildProcessState {
            child: Mutex::new(None),
            pid: AtomicU32::new(0),
            descendant_pid: AtomicU32::new(0),
            descendant_pid_file: dir.path().join("a2-descendant.pid"),
            spawned: Notify::new(),
            hold_before_registration: false,
        });
        let a2 = child_process_runtime(dir.path(), Arc::clone(&a2_state), a2_tx);
        a2.start(controlled_start_params(dir.path(), "exec-real-child-a2"))
            .await
            .expect("A2 starts only after A1 tree retirement succeeds");
        tokio::time::timeout(Duration::from_secs(2), a2_state.spawned.notified())
            .await
            .expect("A2 process tree starts");
        a2.retire().await.expect("A2 process tree retires");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_before_registration_abort_drops_kill_safe_child() {
        let dir = tempfile::tempdir().expect("temp dir creates");
        let (tx, _rx) = mpsc::unbounded_channel();
        let state = Arc::new(ChildProcessState {
            child: Mutex::new(None),
            pid: AtomicU32::new(0),
            descendant_pid: AtomicU32::new(0),
            descendant_pid_file: dir.path().join("pre-registration-descendant.pid"),
            spawned: Notify::new(),
            hold_before_registration: true,
        });
        let runtime = child_process_runtime(dir.path(), Arc::clone(&state), tx);

        runtime
            .start(controlled_start_params(dir.path(), "exec-pre-registration"))
            .await
            .expect("execution starts on generation A1");
        tokio::time::timeout(Duration::from_secs(2), state.spawned.notified())
            .await
            .expect("child reaches the pre-registration barrier");
        let pid = state.pid.load(Ordering::SeqCst);
        let descendant_pid = state.descendant_pid.load(Ordering::SeqCst);
        assert_ne!(pid, 0);
        assert_ne!(descendant_pid, 0);
        assert!(state.child.lock().expect("child process lock").is_none());
        assert!(
            process_is_executable(pid),
            "leader is live before retirement"
        );
        assert!(
            process_is_executable(descendant_pid),
            "descendant is live before retirement"
        );

        let retirement = tokio::time::timeout(Duration::from_secs(9), runtime.retire())
            .await
            .expect("bounded retirement finishes");
        assert!(retirement.is_err(), "aborted task must fail closed");
        tokio::time::timeout(Duration::from_secs(2), async {
            while process_is_executable(pid) || process_is_executable(descendant_pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("drop guard kills the pre-registration process group");
    }

    #[cfg(unix)]
    fn process_is_executable(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            let Some(end_of_command) = stat.rfind(')') else {
                return false;
            };
            !matches!(stat[end_of_command + 2..].chars().next(), Some('Z' | 'X'))
        }
        #[cfg(not(target_os = "linux"))]
        {
            executors::is_pid_alive(pid)
        }
    }

    async fn next_execution_log_line(
        rx: &mut mpsc::UnboundedReceiver<DaemonFrame>,
        execution_id: &str,
        expected_line: &str,
    ) -> ExecutionLogNotification {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let frame = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("execution log arrives")
                .expect("runtime keeps sender open");
            let DaemonFrame::Notification { method, params } = frame else {
                continue;
            };
            if method != METHOD_EXECUTION_LOG {
                continue;
            }
            let notification: ExecutionLogNotification =
                serde_json::from_value(params).expect("execution log parses");
            if notification.execution_id == execution_id && notification.line == expected_line {
                return notification;
            }
        }
    }

    async fn next_terminal_notification(
        rx: &mut mpsc::UnboundedReceiver<DaemonFrame>,
        execution_id: &str,
    ) -> ExecutionTerminalNotification {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let frame = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("terminal notification arrives")
                .expect("runtime keeps sender open");
            let DaemonFrame::Notification { method, params } = frame else {
                continue;
            };
            if method != METHOD_EXECUTION_TERMINAL {
                continue;
            }
            let notification: ExecutionTerminalNotification =
                serde_json::from_value(params).expect("terminal notification parses");
            if notification.execution_id == execution_id {
                return notification;
            }
        }
    }

    #[test]
    fn tracker_lingers_finished_executions_in_active_ids() {
        let tracker = ActiveExecutionTracker::default();
        let guard = tracker.track("exec-1".to_owned());
        assert_eq!(tracker.active_ids(), ["exec-1"]);

        drop(guard);
        assert_eq!(
            tracker.active_ids(),
            ["exec-1"],
            "finished execution must linger in reports until the terminal notification has settled"
        );
    }

    #[test]
    fn tracker_prunes_finished_executions_after_linger() {
        let tracker = ActiveExecutionTracker::with_finished_linger(Duration::ZERO);
        let guard = tracker.track("exec-1".to_owned());
        drop(guard);
        assert!(tracker.active_ids().is_empty());
    }

    #[test]
    fn tracker_retrack_moves_id_back_to_active() {
        let tracker = ActiveExecutionTracker::with_finished_linger(Duration::ZERO);
        let first = tracker.track("exec-1".to_owned());
        drop(first);
        let _second = tracker.track("exec-1".to_owned());
        assert_eq!(tracker.active_ids(), ["exec-1"]);
    }
}
