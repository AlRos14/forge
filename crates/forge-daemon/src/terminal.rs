use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use api_types::{
    DaemonErrorPayload, DaemonFrame, TerminalExitedNotification, TerminalInputParams,
    TerminalInputResult, TerminalOutputNotification, TerminalResizeParams, TerminalResizeResult,
    TerminalStartParams, TerminalStartResult, TerminalTerminateParams, TerminalTerminateResult,
    INVALID_FRAME, INVALID_INPUT, METHOD_TERMINAL_EXITED, METHOD_TERMINAL_OUTPUT,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::{sync::mpsc::UnboundedSender, sync::Mutex, time as tokio_time};

const PATH_GUARDRAIL: &str = "path_guardrail";
const TERMINAL_NOT_FOUND: &str = "terminal_not_found";
const TERMINAL_ERROR: &str = "terminal_error";
const TERMINAL_EXISTS: &str = "terminal_exists";
const MIN_TERMINAL_DIMENSION: u16 = 2;
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

type SharedChild = Arc<StdMutex<Box<dyn Child + Send>>>;

pub struct TerminalRuntime {
    sessions: Mutex<HashMap<String, TerminalSessionHandle>>,
    outbound: UnboundedSender<DaemonFrame>,
    workspace_root: PathBuf,
}

struct TerminalSessionHandle {
    child: SharedChild,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    last_activity: Arc<AtomicU64>,
    started_at: Instant,
    idle_timeout_secs: u64,
    max_lifetime_secs: u64,
}

impl TerminalRuntime {
    pub fn new(outbound: UnboundedSender<DaemonFrame>, workspace_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            outbound,
            workspace_root,
        })
    }

    pub async fn start(
        self: &Arc<Self>,
        params: TerminalStartParams,
    ) -> Result<TerminalStartResult, DaemonErrorPayload> {
        if self.sessions.lock().await.contains_key(&params.session_id) {
            return Err(terminal_exists_error(&params.session_id));
        }

        let workspace_path =
            canonicalize_under_root(&self.workspace_root, Path::new(&params.workspace_path))?;
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(params.rows, params.cols)?)
            .map_err(|error| terminal_error(format!("failed to open PTY: {error}")))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| terminal_error(format!("failed to clone PTY reader: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| terminal_error(format!("failed to take PTY writer: {error}")))?;
        let mut command = command_builder(params.shell);
        command.cwd(workspace_path.as_os_str());
        if let Some(env) = params.env {
            for (key, value) in env {
                command.env(key, value);
            }
        }

        let child = pair.slave.spawn_command(command).map_err(|error| {
            terminal_error(format!("failed to spawn terminal command: {error}"))
        })?;
        let pid = child.process_id();
        let child: Box<dyn Child + Send> = child;
        let child = Arc::new(StdMutex::new(child));
        let now = unix_timestamp_secs();
        let last_activity = Arc::new(AtomicU64::new(now));
        let started_at = Instant::now();
        let started_at_ts = rfc3339_now();
        let session_id = params.session_id;
        let idle_timeout_secs = params.idle_timeout_secs;
        let max_lifetime_secs = params.max_lifetime_secs;

        let handle = TerminalSessionHandle {
            child: Arc::clone(&child),
            writer,
            master: pair.master,
            last_activity: Arc::clone(&last_activity),
            started_at,
            idle_timeout_secs,
            max_lifetime_secs,
        };

        {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&session_id) {
                let _ = child.lock().map(|mut child| child.kill());
                return Err(terminal_exists_error(&session_id));
            }
            sessions.insert(session_id.clone(), handle);
        }

        spawn_reader_task(
            self.outbound.clone(),
            session_id.clone(),
            Arc::clone(&last_activity),
            reader,
        );
        let runtime = Arc::clone(self);
        let waiter_session_id = session_id.clone();
        tokio::spawn(async move {
            runtime.wait_for_child_exit(waiter_session_id).await;
        });
        let runtime = Arc::clone(self);
        let watchdog_session_id = session_id.clone();
        tokio::spawn(async move {
            runtime.watchdog(watchdog_session_id).await;
        });

        Ok(TerminalStartResult {
            session_id,
            pid,
            started_at: started_at_ts,
        })
    }

    pub async fn input(
        &self,
        params: TerminalInputParams,
    ) -> Result<TerminalInputResult, DaemonErrorPayload> {
        let data = STANDARD
            .decode(params.data.as_bytes())
            .map_err(|error| DaemonErrorPayload {
                code: INVALID_FRAME.to_owned(),
                message: format!("invalid terminal input data: {error}"),
                details: None,
            })?;

        let mut sessions = self.sessions.lock().await;
        let handle = sessions
            .get_mut(&params.session_id)
            .ok_or_else(|| terminal_not_found_error(&params.session_id))?;
        handle
            .writer
            .write_all(&data)
            .map_err(|error| terminal_error(format!("failed to write terminal input: {error}")))?;
        handle
            .last_activity
            .store(unix_timestamp_secs(), Ordering::Relaxed);

        Ok(TerminalInputResult {
            session_id: params.session_id,
            accepted: true,
        })
    }

    pub async fn resize(
        &self,
        params: TerminalResizeParams,
    ) -> Result<TerminalResizeResult, DaemonErrorPayload> {
        let sessions = self.sessions.lock().await;
        let handle = sessions
            .get(&params.session_id)
            .ok_or_else(|| terminal_not_found_error(&params.session_id))?;
        handle
            .master
            .resize(pty_size(params.rows, params.cols)?)
            .map_err(|error| terminal_error(format!("failed to resize terminal: {error}")))?;
        handle
            .last_activity
            .store(unix_timestamp_secs(), Ordering::Relaxed);

        Ok(TerminalResizeResult {
            session_id: params.session_id,
            applied: true,
        })
    }

    pub async fn terminate(
        &self,
        params: TerminalTerminateParams,
    ) -> Result<TerminalTerminateResult, DaemonErrorPayload> {
        let session_id = params.session_id;
        let reason = params.reason.unwrap_or_else(|| "terminated".to_owned());
        let terminated = self
            .remove_and_kill(&session_id, Some(reason), None, None)
            .await;

        Ok(TerminalTerminateResult {
            session_id,
            terminated,
        })
    }

    async fn wait_for_child_exit(self: Arc<Self>, session_id: String) {
        let mut interval = tokio_time::interval(EXIT_POLL_INTERVAL);
        loop {
            interval.tick().await;
            let status = {
                let mut sessions = self.sessions.lock().await;
                let Some(handle) = sessions.get_mut(&session_id) else {
                    return;
                };
                let status = match handle.child.lock() {
                    Ok(mut child) => match child.try_wait() {
                        Ok(status) => status,
                        Err(error) => {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "failed to poll terminal child status"
                            );
                            None
                        }
                    },
                    Err(error) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %error,
                            "terminal child lock poisoned"
                        );
                        None
                    }
                };
                status
            };

            if let Some(status) = status {
                tokio_time::sleep(Duration::from_millis(50)).await;
                let exit_code = i32::try_from(status.exit_code()).ok();
                let signal = status.signal().map(ToOwned::to_owned);
                self.remove_and_kill(&session_id, None, exit_code, signal)
                    .await;
                return;
            }
        }
    }

    async fn watchdog(self: Arc<Self>, session_id: String) {
        let mut interval = tokio_time::interval(WATCHDOG_INTERVAL);
        loop {
            interval.tick().await;
            let reason = {
                let sessions = self.sessions.lock().await;
                let Some(handle) = sessions.get(&session_id) else {
                    return;
                };
                let now = unix_timestamp_secs();
                let last_activity = handle.last_activity.load(Ordering::Relaxed);
                if handle.idle_timeout_secs > 0
                    && now.saturating_sub(last_activity) >= handle.idle_timeout_secs
                {
                    Some("idle_timeout")
                } else if handle.max_lifetime_secs > 0
                    && handle.started_at.elapsed().as_secs() >= handle.max_lifetime_secs
                {
                    Some("max_lifetime")
                } else {
                    None
                }
            };

            if let Some(reason) = reason {
                self.remove_and_kill(&session_id, Some(reason.to_owned()), None, None)
                    .await;
                return;
            }
        }
    }

    async fn remove_and_kill(
        &self,
        session_id: &str,
        reason: Option<String>,
        exit_code: Option<i32>,
        signal: Option<String>,
    ) -> bool {
        let handle = self.sessions.lock().await.remove(session_id);
        let Some(handle) = handle else {
            return false;
        };

        if reason.is_some() {
            match handle.child.lock() {
                Ok(mut child) => {
                    if let Err(error) = child.kill() {
                        tracing::warn!(
                            session_id,
                            error = %error,
                            "failed to kill terminal child"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        session_id,
                        error = %error,
                        "terminal child lock poisoned during termination"
                    );
                }
            }
        }

        self.emit_exited(session_id.to_owned(), exit_code, signal, reason);
        true
    }

    fn emit_exited(
        &self,
        session_id: String,
        exit_code: Option<i32>,
        signal: Option<String>,
        reason: Option<String>,
    ) {
        let notification = TerminalExitedNotification {
            session_id,
            exit_code,
            signal,
            reason,
            ts: rfc3339_now(),
        };
        send_notification(&self.outbound, METHOD_TERMINAL_EXITED, &notification);
    }
}

fn spawn_reader_task(
    outbound: UnboundedSender<DaemonFrame>,
    session_id: String,
    last_activity: Arc<AtomicU64>,
    mut reader: Box<dyn Read + Send>,
) {
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => {
                    last_activity.store(unix_timestamp_secs(), Ordering::Relaxed);
                    let notification = TerminalOutputNotification {
                        session_id: session_id.clone(),
                        data: STANDARD.encode(&buffer[..read]),
                        ts: rfc3339_now(),
                    };
                    if !send_notification(&outbound, METHOD_TERMINAL_OUTPUT, &notification) {
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "terminal PTY reader stopped"
                    );
                    return;
                }
            }
        }
    });
}

fn send_notification<T: Serialize>(
    outbound: &UnboundedSender<DaemonFrame>,
    method: &str,
    params: &T,
) -> bool {
    let params = match serde_json::to_value(params) {
        Ok(params) => params,
        Err(error) => {
            tracing::warn!(
                method,
                error = %error,
                "failed to serialize terminal notification"
            );
            return false;
        }
    };

    outbound
        .send(DaemonFrame::Notification {
            method: method.to_owned(),
            params,
        })
        .is_ok()
}

fn command_builder(shell: Option<String>) -> CommandBuilder {
    let shell = shell.unwrap_or_else(default_shell);
    if shell.split_whitespace().nth(1).is_some() {
        command_builder_for_command_line(shell)
    } else {
        CommandBuilder::new(shell)
    }
}

#[cfg(unix)]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

#[cfg(windows)]
fn default_shell() -> String {
    "cmd.exe".to_owned()
}

#[cfg(unix)]
fn command_builder_for_command_line(command_line: String) -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(command_line);
    command
}

#[cfg(windows)]
fn command_builder_for_command_line(command_line: String) -> CommandBuilder {
    let mut command = CommandBuilder::new("cmd.exe");
    command.arg("/C");
    command.arg(command_line);
    command
}

fn pty_size(rows: u16, cols: u16) -> Result<PtySize, DaemonErrorPayload> {
    if rows < MIN_TERMINAL_DIMENSION || cols < MIN_TERMINAL_DIMENSION {
        return Err(invalid_input_error(format!(
            "terminal rows and cols must each be at least {MIN_TERMINAL_DIMENSION}"
        )));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn rfc3339_now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn canonicalize_under_root(root: &Path, requested: &Path) -> Result<PathBuf, DaemonErrorPayload> {
    let requested = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let canonical_root = root.canonicalize().map_err(|error| {
        path_guardrail_error(format!(
            "failed to resolve daemon workspace root '{}': {error}",
            root.display()
        ))
    })?;
    let canonical_requested = requested.canonicalize().map_err(|error| {
        path_guardrail_error(format!(
            "failed to resolve terminal workspace path '{}': {error}",
            requested.display()
        ))
    })?;

    if !canonical_requested.starts_with(&canonical_root) {
        return Err(path_guardrail_error(format!(
            "terminal workspace path '{}' escapes the daemon's workspace root",
            requested.display()
        )));
    }

    Ok(canonical_requested)
}

fn terminal_not_found_error(session_id: &str) -> DaemonErrorPayload {
    DaemonErrorPayload {
        code: TERMINAL_NOT_FOUND.to_owned(),
        message: format!("terminal session '{session_id}' was not found"),
        details: None,
    }
}

fn terminal_exists_error(session_id: &str) -> DaemonErrorPayload {
    DaemonErrorPayload {
        code: TERMINAL_EXISTS.to_owned(),
        message: format!("terminal session '{session_id}' already exists"),
        details: None,
    }
}

fn terminal_error(message: impl Into<String>) -> DaemonErrorPayload {
    DaemonErrorPayload {
        code: TERMINAL_ERROR.to_owned(),
        message: message.into(),
        details: None,
    }
}

fn invalid_input_error(message: impl Into<String>) -> DaemonErrorPayload {
    DaemonErrorPayload {
        code: INVALID_INPUT.to_owned(),
        message: message.into(),
        details: None,
    }
}

fn path_guardrail_error(message: impl Into<String>) -> DaemonErrorPayload {
    DaemonErrorPayload {
        code: PATH_GUARDRAIL.to_owned(),
        message: message.into(),
        details: None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use api_types::{
        DaemonFrame, TerminalExitedNotification, TerminalInputParams, TerminalOutputNotification,
        TerminalResizeParams, TerminalStartParams, TerminalTerminateParams, INVALID_INPUT,
        METHOD_TERMINAL_EXITED, METHOD_TERMINAL_OUTPUT,
    };
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use tempfile::TempDir;
    use tokio::{sync::mpsc, time::timeout};

    use super::{TerminalRuntime, PATH_GUARDRAIL, TERMINAL_NOT_FOUND};

    #[tokio::test]
    async fn test_start_returns_pid() {
        let (runtime, _rx, _root, workspace) = runtime_with_workspace();
        let result = runtime
            .start(start_params("start-pid", &workspace, None))
            .await
            .expect("terminal starts");

        assert!(result.pid.unwrap_or_default() > 0);
        runtime
            .terminate(TerminalTerminateParams {
                session_id: "start-pid".to_owned(),
                reason: None,
            })
            .await
            .expect("terminate");
    }

    #[tokio::test]
    async fn test_path_escape_rejected() {
        let (runtime, _rx, _root, _workspace) = runtime_with_workspace();
        let outside = TempDir::new().expect("outside tempdir");
        let error = runtime
            .start(start_params("escape", outside.path(), None))
            .await
            .expect_err("path escape rejected");

        assert_eq!(error.code, PATH_GUARDRAIL);
    }

    #[tokio::test]
    async fn test_start_rejects_too_small_terminal_size() {
        let (runtime, _rx, _root, workspace) = runtime_with_workspace();
        let mut params = start_params("small-start", &workspace, None);
        params.rows = 1;

        let error = runtime
            .start(params)
            .await
            .expect_err("too-small terminal size rejected");

        assert_eq!(error.code, INVALID_INPUT);
    }

    #[tokio::test]
    async fn test_resize_ok() {
        let (runtime, _rx, _root, workspace) = runtime_with_workspace();
        runtime
            .start(start_params("resize", &workspace, None))
            .await
            .expect("terminal starts");

        let result = runtime
            .resize(TerminalResizeParams {
                session_id: "resize".to_owned(),
                rows: 40,
                cols: 120,
            })
            .await
            .expect("resize");

        assert!(result.applied);
        runtime
            .terminate(TerminalTerminateParams {
                session_id: "resize".to_owned(),
                reason: None,
            })
            .await
            .expect("terminate");
    }

    #[tokio::test]
    async fn test_resize_rejects_too_small_terminal_size() {
        let (runtime, _rx, _root, workspace) = runtime_with_workspace();
        runtime
            .start(start_params("resize-small", &workspace, None))
            .await
            .expect("terminal starts");

        let error = runtime
            .resize(TerminalResizeParams {
                session_id: "resize-small".to_owned(),
                rows: 40,
                cols: 1,
            })
            .await
            .expect_err("too-small resize rejected");

        assert_eq!(error.code, INVALID_INPUT);
        runtime
            .terminate(TerminalTerminateParams {
                session_id: "resize-small".to_owned(),
                reason: None,
            })
            .await
            .expect("terminate");
    }

    #[tokio::test]
    async fn test_terminate_idempotent() {
        let (runtime, _rx, _root, workspace) = runtime_with_workspace();
        runtime
            .start(start_params("terminate-idempotent", &workspace, None))
            .await
            .expect("terminal starts");

        let first = runtime
            .terminate(TerminalTerminateParams {
                session_id: "terminate-idempotent".to_owned(),
                reason: None,
            })
            .await
            .expect("first terminate");
        let second = runtime
            .terminate(TerminalTerminateParams {
                session_id: "terminate-idempotent".to_owned(),
                reason: None,
            })
            .await
            .expect("second terminate");

        assert!(first.terminated);
        assert!(!second.terminated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_child_exit_notification() {
        let (runtime, mut rx, _root, workspace) = runtime_with_workspace();
        runtime
            .start(start_params(
                "child-exit",
                &workspace,
                Some("sh -c 'echo HELLO; exit 0'"),
            ))
            .await
            .expect("terminal starts");

        timeout(Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match rx.recv().await.expect("notification frame") {
                    DaemonFrame::Notification { method, params }
                        if method == METHOD_TERMINAL_OUTPUT =>
                    {
                        let notification: TerminalOutputNotification =
                            serde_json::from_value(params).expect("terminal output");
                        output.extend(
                            STANDARD
                                .decode(notification.data.as_bytes())
                                .expect("decode output"),
                        );
                    }
                    DaemonFrame::Notification { method, params }
                        if method == METHOD_TERMINAL_EXITED =>
                    {
                        let notification: TerminalExitedNotification =
                            serde_json::from_value(params).expect("terminal exited");
                        assert_eq!(notification.session_id, "child-exit");
                        assert!(
                            String::from_utf8_lossy(&output).contains("HELLO"),
                            "output before exit notification should contain HELLO"
                        );
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("exit notification");
    }

    #[tokio::test]
    async fn test_input_missing_session() {
        let (runtime, _rx, _root, _workspace) = runtime_with_workspace();
        let error = runtime
            .input(TerminalInputParams {
                session_id: "missing".to_owned(),
                data: STANDARD.encode("hello"),
            })
            .await
            .expect_err("missing session");

        assert_eq!(error.code, TERMINAL_NOT_FOUND);
    }

    fn runtime_with_workspace() -> (
        std::sync::Arc<TerminalRuntime>,
        mpsc::UnboundedReceiver<DaemonFrame>,
        TempDir,
        std::path::PathBuf,
    ) {
        let root = TempDir::new().expect("workspace root");
        let workspace = root.path().join("worktree");
        std::fs::create_dir_all(&workspace).expect("workspace subdir");
        let (tx, rx) = mpsc::unbounded_channel();
        let runtime = TerminalRuntime::new(tx, root.path().to_path_buf());
        (runtime, rx, root, workspace)
    }

    fn start_params(
        session_id: &str,
        workspace_path: &std::path::Path,
        shell: Option<&str>,
    ) -> TerminalStartParams {
        TerminalStartParams {
            session_id: session_id.to_owned(),
            workspace_path: workspace_path.to_string_lossy().into_owned(),
            rows: 24,
            cols: 80,
            shell: shell.map(ToOwned::to_owned),
            env: None,
            idle_timeout_secs: 1_800,
            max_lifetime_secs: 28_800,
        }
    }
}
