#![cfg(unix)]

use std::{
    io::{Read, Write},
    net::SocketAddr,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use api_types::LoginRequest;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use portable_pty::{native_pty_system, Child, CommandBuilder, ExitStatus, PtySize};

const EMAIL: &str = "pty-test@example.com";
const PASSWORD: &str = "pty-secret-value";
const SENTINEL: &str = "stdin-sentinel-never-authenticate";
const TIMEOUT: Duration = Duration::from_secs(15);
const RESULT_MARKER: &str = "__FORGE_PTY_RESULT__";
const LOGIN_SCRIPT: &str = r#"
before=$(stty -g)
"$FORGE_TEST_BINARY" --server "$FORGE_TEST_SERVER" login --email "pty-test@example.com"
code=$?
after=$(stty -g)
printf '\n__FORGE_PTY_RESULT__|%s|%s|%s\n' "$code" "$before" "$after"
exit "$code"
"#;

#[test]
fn interactive_success_hides_password_and_restores_terminal_state() {
    let server = MockServer::spawn(false);
    let data_dir = tempfile::tempdir().expect("temporary Forge data dir");
    let mut process = PtyProcess::spawn(&server, data_dir.path());

    process.wait_for_prompt();
    process.send(format!("{PASSWORD}\r").as_bytes());
    let (status, transcript) = process.finish("interactive successful login");

    assert!(status.success());
    assert_eq!(server.login_requests(), 1);
    assert_eq!(server.received_password().as_deref(), Some(PASSWORD));
    assert_terminal_restored(&transcript);
    assert_no_secret(&transcript);
}

#[test]
fn authentication_rejection_hides_password_and_restores_terminal_state() {
    let server = MockServer::spawn(true);
    let data_dir = tempfile::tempdir().expect("temporary Forge data dir");
    let mut process = PtyProcess::spawn(&server, data_dir.path());

    process.wait_for_prompt();
    process.send(format!("{PASSWORD}\r").as_bytes());
    let (status, transcript) = process.finish("interactive rejected login");

    assert!(!status.success());
    assert_eq!(server.login_requests(), 1);
    assert_eq!(server.received_password().as_deref(), Some(PASSWORD));
    assert_terminal_restored(&transcript);
    assert_no_secret(&transcript);
}

#[test]
fn empty_eof_does_not_authenticate_and_restores_terminal_state() {
    let server = MockServer::spawn(false);
    let data_dir = tempfile::tempdir().expect("temporary Forge data dir");
    let mut process = PtyProcess::spawn(&server, data_dir.path());

    process.wait_for_prompt();
    process.send(&[0x04]);
    let (status, transcript) = process.finish("interactive EOF login");

    assert!(!status.success());
    assert_eq!(server.login_requests(), 0);
    assert_terminal_restored(&transcript);
    assert_no_secret(&transcript);
}

#[test]
fn ctrl_c_does_not_authenticate_and_restores_terminal_state() {
    let server = MockServer::spawn(false);
    let data_dir = tempfile::tempdir().expect("temporary Forge data dir");
    let mut process = PtyProcess::spawn(&server, data_dir.path());

    process.wait_for_prompt();
    process.send(&[0x03]);
    let (status, transcript) = process.finish("interactive Ctrl-C login");

    assert!(!status.success());
    assert_eq!(server.login_requests(), 0);
    assert_terminal_restored(&transcript);
    assert_no_secret(&transcript);
}

#[test]
fn implicit_non_tty_fails_before_reading_or_authenticating() {
    let server = MockServer::spawn(false);
    let data_dir = tempfile::tempdir().expect("temporary Forge data dir");
    let mut child = Command::new(forge_ctl_binary())
        .args(["--server", &server.url(), "login", "--email", EMAIL])
        .env("FORGE_DATA_DIR", data_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn non-TTY login");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin
        .write_all(SENTINEL.as_bytes())
        .expect("write non-TTY sentinel");
    drop(stdin);

    let output = wait_for_process_output(child, "implicit non-TTY login");

    assert!(!output.status.success());
    assert_eq!(server.login_requests(), 0);
    assert!(String::from_utf8_lossy(&output.stderr).contains("--password-stdin"));
    assert_no_secret(&output.stdout);
    assert_no_secret(&output.stderr);
}

#[test]
fn password_stdin_remains_supported_without_echoing_password() {
    let server = MockServer::spawn(false);
    let data_dir = tempfile::tempdir().expect("temporary Forge data dir");
    let mut child = Command::new(forge_ctl_binary())
        .args([
            "--server",
            &server.url(),
            "login",
            "--email",
            EMAIL,
            "--password-stdin",
        ])
        .env("FORGE_DATA_DIR", data_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn password-stdin login");
    let mut stdin = child.stdin.take().expect("piped stdin");
    writeln!(stdin, "{PASSWORD}").expect("write stdin password");
    drop(stdin);

    let output = wait_for_process_output(child, "password-stdin login");

    assert!(output.status.success());
    assert_eq!(server.login_requests(), 1);
    assert_eq!(server.received_password().as_deref(), Some(PASSWORD));
    assert_no_secret(&output.stdout);
    assert_no_secret(&output.stderr);
}

fn forge_ctl_binary() -> &'static str {
    env!("CARGO_BIN_EXE_forge-ctl")
}

fn assert_no_secret(output: &[u8]) {
    assert!(
        !contains(output, PASSWORD.as_bytes()),
        "password was emitted"
    );
    assert!(
        !contains(output, SENTINEL.as_bytes()),
        "sentinel was emitted"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn assert_terminal_restored(transcript: &[u8]) {
    let transcript = String::from_utf8_lossy(transcript);
    let result = transcript
        .lines()
        .find_map(|line| line.trim().strip_prefix(RESULT_MARKER))
        .expect("terminal state result marker is present");
    let mut fields = result.trim_start_matches('|').splitn(3, '|');
    let _exit_code = fields.next().expect("result exit code");
    let before = fields.next().expect("terminal state before login");
    let after = fields.next().expect("terminal state after login");

    assert_eq!(
        normalized_terminal_state(before),
        normalized_terminal_state(after),
        "terminal configuration was not restored"
    );
}

#[cfg(target_os = "macos")]
fn normalized_terminal_state(state: &str) -> String {
    // PENDIN (0x20000000) is a volatile kernel state flag, not terminal
    // configuration. macOS can set it while reprocessing queued input
    // during a raw/canonical transition.
    state
        .split(':')
        .map(|field| {
            field.strip_prefix("lflag=").map_or_else(
                || field.to_owned(),
                |value| {
                    let flags =
                        u64::from_str_radix(value, 16).expect("macOS stty lflag is hexadecimal");
                    format!("lflag={:x}", flags & !0x2000_0000)
                },
            )
        })
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(not(target_os = "macos"))]
fn normalized_terminal_state(state: &str) -> String {
    state.to_owned()
}

fn wait_for_process_output(
    mut child: std::process::Child,
    description: &str,
) -> std::process::Output {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().unwrap_or_else(|error| {
                    panic!("could not collect {description} output: {error}")
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{description} timed out");
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("could not poll {description}: {error}");
            }
        }
    }
}

struct PtyProcess {
    child: Box<dyn Child + Send + Sync>,
    writer: Option<Box<dyn Write + Send>>,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
}

impl PtyProcess {
    fn spawn(server: &MockServer, data_dir: &Path) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");
        let mut command = CommandBuilder::new("sh");
        command.arg("-c");
        command.arg(LOGIN_SCRIPT);
        command.env("FORGE_TEST_BINARY", forge_ctl_binary());
        command.env("FORGE_TEST_SERVER", server.url());
        command.env("FORGE_DATA_DIR", data_dir);
        let child = pair
            .slave
            .spawn_command(command)
            .expect("spawn login in PTY");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
        let writer = pair.master.take_writer().expect("take PTY writer");
        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let reader_handle = thread::spawn(move || {
            let mut buffer = [0_u8; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => reader_output
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .extend_from_slice(&buffer[..read]),
                }
            }
        });

        Self {
            child,
            writer: Some(writer),
            output,
            reader: Some(reader_handle),
        }
    }

    fn wait_for_prompt(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let has_prompt = {
                let output = self
                    .output
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                contains(&output, b"Password: ")
            };
            if has_prompt {
                return;
            }
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                self.kill_and_reap();
                panic!("login exited before showing its password prompt");
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.kill_and_reap();
        panic!("password prompt timed out");
    }

    fn send(&mut self, bytes: &[u8]) {
        let writer = self.writer.as_mut().expect("PTY writer is available");
        writer.write_all(bytes).expect("write PTY input");
        writer.flush().expect("flush PTY input");
    }

    fn finish(mut self, description: &str) -> (ExitStatus, Vec<u8>) {
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    self.kill_and_reap();
                    panic!("{description} timed out");
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    self.kill_and_reap();
                    panic!("could not poll {description}: {error}");
                }
            }
        };
        let _ = self.child.wait();
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let output = self
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        (status, output)
    }

    fn kill_and_reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill_and_reap();
        }
    }
}

#[derive(Clone)]
struct MockState {
    reject_login: bool,
    login_requests: Arc<AtomicUsize>,
    received_password: Arc<Mutex<Option<String>>>,
}

struct MockServer {
    addr: SocketAddr,
    state: MockState,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    fn spawn(reject_login: bool) -> Self {
        let state = MockState {
            reject_login,
            login_requests: Arc::new(AtomicUsize::new(0)),
            received_password: Arc::new(Mutex::new(None)),
        };
        let router = Router::new()
            .route("/api/v1/auth/login", post(login_route))
            .route("/api/v1/auth/tokens", post(token_route))
            .with_state(state.clone());
        let (addr_sender, addr_receiver) = mpsc::sync_channel(1);
        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
        let server_thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build mock server runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind mock login server");
                addr_sender
                    .send(listener.local_addr().expect("mock server address"))
                    .expect("send mock server address");
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_receiver.await;
                    })
                    .await
                    .expect("serve mock login API");
            });
        });
        let addr = addr_receiver
            .recv_timeout(TIMEOUT)
            .expect("receive mock server address");

        Self {
            addr,
            state,
            shutdown: Some(shutdown_sender),
            thread: Some(server_thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn login_requests(&self) -> usize {
        self.state.login_requests.load(Ordering::SeqCst)
    }

    fn received_password(&self) -> Option<String> {
        self.state
            .received_password
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn login_route(
    State(state): State<MockState>,
    Json(request): Json<LoginRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    state.login_requests.fetch_add(1, Ordering::SeqCst);
    {
        let mut received = state
            .received_password
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *received = Some(request.password);
    }
    if state.reject_login {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "authentication rejected"})),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "token_type": "bearer",
            "expires_in": 3600
        })),
    )
}

async fn token_route() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": "token-id",
        "name": "forge-ctl",
        "token": "stored-personal-access-token",
        "prefix": "stored",
        "scopes": "*",
        "expires_at": null,
        "last_used_at": null,
        "created_at": "2026-08-09T00:00:00Z"
    }))
}
