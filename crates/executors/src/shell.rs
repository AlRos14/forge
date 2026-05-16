use crate::{
    ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError, LogKind, LogStream,
    LogWriter, TaskExecutor,
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
    process::{Child, Command},
    sync::{mpsc, Mutex as AsyncMutex},
    time::{self, MissedTickBehavior},
};

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct ShellExecutor {
    processes: Arc<Mutex<HashMap<String, Arc<RunningProcess>>>>,
}

struct RunningProcess {
    child: AsyncMutex<Child>,
    cancellation_requested: AtomicBool,
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

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(&ctx.description)
            .current_dir(&ctx.worktree_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(max_turns) = ctx.max_turns {
            command.env("FORGE_MAX_TURNS", max_turns.to_string());
        }
        let mut child = command.spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExecutorError::Other("failed to capture child stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ExecutorError::Other("failed to capture child stderr".to_string()))?;

        let process = Arc::new(RunningProcess {
            child: AsyncMutex::new(child),
            cancellation_requested: AtomicBool::new(false),
        });

        self.insert_process(ctx.execution_id.clone(), process.clone())?;

        let result = self
            .supervise_process(&ctx, process.clone(), stdout, stderr, &mut writer)
            .await;

        self.remove_process(&ctx.execution_id)?;

        result
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let Some(process) = self.get_process(execution_id)? else {
            return Ok(());
        };

        process.cancellation_requested.store(true, Ordering::SeqCst);

        let child_id = {
            let child = process.child.lock().await;
            child.id()
        };

        if let Some(child_id) = child_id {
            let _ = send_sigterm(child_id).await;
        }

        let deadline = time::Instant::now() + CANCEL_GRACE_PERIOD;
        loop {
            {
                let mut child = process.child.lock().await;
                if child.try_wait()?.is_some() {
                    return Ok(());
                }
            }

            if time::Instant::now() >= deadline {
                break;
            }

            time::sleep(COMPLETION_POLL_INTERVAL).await;
        }

        let mut child = process.child.lock().await;
        child.start_kill()?;

        Ok(())
    }
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
                        child.try_wait()?
                    };

                    if let Some(status) = status {
                        break status;
                    }
                }
                _ = heartbeat.tick() => {
                    let status = {
                        let mut child = process.child.lock().await;
                        child.try_wait()?
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
            })
        } else {
            Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: Some(format!("shell command exited with status {status}")),
                usage: None,
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

async fn send_sigterm(pid: u32) -> std::io::Result<()> {
    Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .await
        .map(|_| ())
}
