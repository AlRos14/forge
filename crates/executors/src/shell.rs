use crate::{
    build_shell_command_plan, ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError,
    LogKind, LogStream, LogWriter, ProcessGroupChild, ShellConfig, TaskExecutor,
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::{mpsc, Mutex as AsyncMutex},
    time::{self, MissedTickBehavior},
};

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct ShellExecutor {
    processes: Arc<Mutex<HashMap<String, Arc<RunningProcess>>>>,
    cancel_grace_period: Duration,
    shell_program: Option<String>,
}

impl Default for ShellExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellExecutor {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
            cancel_grace_period: DEFAULT_CANCEL_GRACE_PERIOD,
            shell_program: None,
        }
    }

    pub fn with_cancel_grace_period(mut self, grace: Duration) -> Self {
        self.cancel_grace_period = grace;
        self
    }

    #[doc(hidden)]
    pub fn with_shell_program(mut self, program: impl Into<String>) -> Self {
        self.shell_program = Some(program.into());
        self
    }

    #[doc(hidden)]
    pub fn has_process(&self, execution_id: &str) -> bool {
        self.lock_processes()
            .map(|processes| processes.contains_key(execution_id))
            .unwrap_or(false)
    }

    #[doc(hidden)]
    pub async fn running_child_pid(&self, execution_id: &str) -> Option<u32> {
        let process = self.get_process(execution_id).ok()??;
        let child = process.child.lock().await;
        child.id()
    }
}

struct RunningProcess {
    child: Arc<AsyncMutex<ProcessGroupChild>>,
    cancellation_requested: AtomicBool,
}

struct GroupKillOnDrop {
    child: Arc<AsyncMutex<ProcessGroupChild>>,
    armed: bool,
}

impl GroupKillOnDrop {
    fn new(child: Arc<AsyncMutex<ProcessGroupChild>>) -> Self {
        Self { child, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GroupKillOnDrop {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut child) = self.child.try_lock() {
                let _ = child.start_kill();
            }
        }
    }
}

enum OutputEvent {
    Line { kind: LogKind, line: String },
    ReaderError { kind: LogKind, error: String },
}

#[async_trait]
impl TaskExecutor for ShellExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let mut writer = LogWriter::new(
            &ctx.logs_path,
            ctx.execution_id.clone(),
            DEFAULT_MAX_OUTPUT_BYTES,
        );
        if let Some(sender) = ctx.log_sender.clone() {
            writer.set_log_sender(sender);
        }

        let shell_config: ShellConfig =
            serde_json::from_value(ctx.agent_config.clone()).unwrap_or_default();
        let mut plan = build_shell_command_plan(
            &ctx.description,
            &ctx.worktree_path,
            ctx.max_turns,
            Some(&shell_config),
        );
        if let Some(shell_program) = &self.shell_program {
            plan.program = shell_program.clone();
        }

        let mut command = Command::new(&plan.program);
        command
            .args(&plan.args)
            .current_dir(&plan.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in &plan.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &plan.env_set {
            command.env(key, value);
        }
        command.kill_on_drop(true);

        let mut child = match ProcessGroupChild::spawn(&mut command) {
            Ok(child) => child,
            Err(error) => {
                return Err(ExecutorError::Io(error));
            }
        };

        let Some(stdout) = child.inner().stdout.take() else {
            let cleanup = terminate_process_group(&mut child).await;
            return Err(cleanup.err().map_or_else(
                || ExecutorError::Other("failed to capture child stdout".to_string()),
                ExecutorError::Io,
            ));
        };
        let Some(stderr) = child.inner().stderr.take() else {
            let cleanup = terminate_process_group(&mut child).await;
            return Err(cleanup.err().map_or_else(
                || ExecutorError::Other("failed to capture child stderr".to_string()),
                ExecutorError::Io,
            ));
        };

        let process = Arc::new(RunningProcess {
            child: Arc::new(AsyncMutex::new(child)),
            cancellation_requested: AtomicBool::new(false),
        });
        let mut group_kill_guard = GroupKillOnDrop::new(Arc::clone(&process.child));

        if let Err(registration_error) =
            self.insert_process(ctx.execution_id.clone(), process.clone())
        {
            let cleanup = {
                let mut child = process.child.lock().await;
                terminate_process_group(&mut child).await
            };
            if let Err(cleanup_error) = cleanup {
                return Err(ExecutorError::Other(format!(
                    "shell process registration failed ({registration_error}); process group termination failed ({cleanup_error})"
                )));
            }
            group_kill_guard.disarm();
            return Err(registration_error);
        }

        let result = self
            .supervise_process(&ctx, process.clone(), stdout, stderr, &mut writer)
            .await;
        let cleanup = {
            let mut child = process.child.lock().await;
            terminate_process_group(&mut child).await
        };
        if let Err(cleanup_error) = cleanup {
            return Err(ExecutorError::Other(format!(
                "shell process-group termination could not be verified: {cleanup_error}"
            )));
        }
        group_kill_guard.disarm();

        self.remove_process(&ctx.execution_id)?;

        result
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        // Idempotent: the process may have finished (and been reaped) between the
        // caller's decision to cancel and this call.
        let Some(process) = self.get_process(execution_id)? else {
            return Ok(());
        };

        process.cancellation_requested.store(true, Ordering::SeqCst);

        #[cfg(unix)]
        {
            process.child.lock().await.send_sigterm()?;
            let deadline = time::Instant::now() + self.cancel_grace_period;
            let process_group_exited = loop {
                let alive = process.child.lock().await.group_is_alive()?;
                if !alive {
                    break true;
                }
                if time::Instant::now() >= deadline {
                    break false;
                }
                time::sleep(COMPLETION_POLL_INTERVAL).await;
            };

            let mut child = process.child.lock().await;
            if process_group_exited {
                child.wait_leader().await?;
            } else {
                terminate_process_group(&mut child).await?;
            }
        }
        #[cfg(not(unix))]
        {
            let mut child = process.child.lock().await;
            terminate_process_group(&mut child).await?;
        }

        Ok(())
    }
}

async fn terminate_process_group(child: &mut ProcessGroupChild) -> std::io::Result<()> {
    child.kill_and_wait().await.map(|_| ())
}

impl ShellExecutor {
    fn insert_process(
        &self,
        execution_id: String,
        process: Arc<RunningProcess>,
    ) -> Result<(), ExecutorError> {
        let mut processes = self.lock_processes()?;
        processes.insert(execution_id, process);
        Ok(())
    }

    fn get_process(
        &self,
        execution_id: &str,
    ) -> Result<Option<Arc<RunningProcess>>, ExecutorError> {
        let processes = self.lock_processes()?;
        Ok(processes.get(execution_id).cloned())
    }

    fn remove_process(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let mut processes = self.lock_processes()?;
        processes.remove(execution_id);
        Ok(())
    }

    fn lock_processes(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, Arc<RunningProcess>>>, ExecutorError>
    {
        self.processes
            .lock()
            .map_err(|_| ExecutorError::Other("shell process map lock poisoned".to_string()))
    }

    async fn supervise_process(
        &self,
        ctx: &ExecutionContext,
        process: Arc<RunningProcess>,
        stdout: impl AsyncRead + Unpin + Send + 'static,
        stderr: impl AsyncRead + Unpin + Send + 'static,
        writer: &mut LogWriter,
    ) -> Result<ExecutionResult, ExecutorError> {
        let (tx, mut rx) = mpsc::channel(256);
        tokio::spawn(read_output_lines(stdout, LogKind::Stdout, tx.clone()));
        tokio::spawn(read_output_lines(stderr, LogKind::Stderr, tx));

        let mut completion_interval = time::interval(COMPLETION_POLL_INTERVAL);
        completion_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let heartbeat_interval = Duration::from_secs(ctx.heartbeat_interval_seconds.max(1));
        let mut heartbeat = time::interval(heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let status = loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    write_output_event(writer, event).await?;
                }
                _ = completion_interval.tick() => {
                    let status = {
                        let mut child = process.child.lock().await;
                        child.try_wait_leader()?
                    };

                    if let Some(status) = status {
                        break status;
                    }
                }
                _ = heartbeat.tick() => {
                    let status = {
                        let mut child = process.child.lock().await;
                        child.try_wait_leader()?
                    };

                    if let Some(status) = status {
                        break status;
                    }

                    writer
                        .write(
                            LogKind::System,
                            LogStream::Heartbeat,
                            serde_json::json!({
                                "status": "alive",
                                "task_id": ctx.task_id,
                                "execution_id": ctx.execution_id,
                            }),
                        )
                        .await?;
                }
            }
        };

        while let Some(event) = rx.recv().await {
            write_output_event(writer, event).await?;
        }

        if process.cancellation_requested.load(Ordering::SeqCst) {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Cancelled,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
                ..Default::default()
            });
        }

        if status.success() {
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
                ..Default::default()
            })
        } else {
            Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: Some(format!("shell command exited with status {status}")),
                usage: None,
                ..Default::default()
            })
        }
    }
}

async fn read_output_lines<R>(reader: R, kind: LogKind, tx: mpsc::Sender<OutputEvent>)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if tx
                    .send(OutputEvent::Line {
                        kind: kind.clone(),
                        line,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = tx
                    .send(OutputEvent::ReaderError {
                        kind,
                        error: error.to_string(),
                    })
                    .await;
                break;
            }
        }
    }
}

async fn write_output_event(
    writer: &mut LogWriter,
    event: OutputEvent,
) -> Result<(), ExecutorError> {
    match event {
        OutputEvent::Line { kind, line } => {
            writer
                .write(kind, LogStream::Main, serde_json::json!({ "line": line }))
                .await?;
        }
        OutputEvent::ReaderError { kind, error } => {
            writer
                .write(
                    LogKind::System,
                    LogStream::Main,
                    serde_json::json!({
                        "error": error,
                        "source": match kind {
                            LogKind::Stdout => "stdout",
                            LogKind::Stderr => "stderr",
                            _ => "output",
                        },
                    }),
                )
                .await?;
        }
    }

    Ok(())
}

#[cfg(unix)]
#[doc(hidden)]
pub fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
#[doc(hidden)]
pub fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
