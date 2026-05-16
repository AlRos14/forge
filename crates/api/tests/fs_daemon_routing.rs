#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use api::{build_router, serve_with_listener, AppState};
use api_types::{
    BranchListResponse, DaemonFrame, DaemonRegisterResponse, ErrorResponse,
    ExecutionTerminalNotification, FsEntry, FsListResponse, FsListResult,
    METHOD_EXECUTION_TERMINAL, METHOD_FS_LIST,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use db::{
    AgentRepo, AgentStatus, CreateAgent, CreateExecution, CreateProject, CreateTask, DaemonRepo,
    DaemonStatus, Execution, ExecutionRepo, ExecutionStatus, ProjectRepo, TaskRepo, UpsertDaemon,
    UserRepo,
};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest, http::HeaderValue, Error as WsError, Message as WsMessage,
    },
    MaybeTlsStream, WebSocketStream,
};
use tower::ServiceExt;

const TEST_USER_ID: &str = "test-user-id";
type ClientSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[tokio::test]
async fn embedded_daemon_lists_local_filesystem() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-embedded");
    fs::write(dir.path().join("alpha.txt"), "alpha\n").expect("write alpha");

    let response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?daemon_id={daemon_id}&path={}",
            query_path(dir.path())
        ),
        StatusCode::OK,
    )
    .await;

    assert_eq!(
        response.path,
        dir.path()
            .canonicalize()
            .expect("temp dir canonicalizes")
            .to_string_lossy()
    );
    assert!(response
        .entries
        .iter()
        .any(|entry| { entry.name == "alpha.txt" && !entry.is_dir && !entry.is_git_repo }));
}

#[tokio::test]
async fn embedded_daemon_marks_git_repo_directories() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-git-mark");
    let git_dir = dir.path().join("git-repo");
    let plain_dir = dir.path().join("plain-dir");
    fs::create_dir_all(&git_dir).expect("create git dir");
    fs::create_dir_all(&plain_dir).expect("create plain dir");
    run_git(&git_dir, &["init"]);

    let response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            query_path(dir.path())
        ),
        StatusCode::OK,
    )
    .await;

    let git_entry = response
        .entries
        .iter()
        .find(|entry| entry.name == "git-repo")
        .expect("git entry exists");
    assert!(git_entry.is_dir);
    assert!(git_entry.is_git_repo);

    let plain_entry = response
        .entries
        .iter()
        .find(|entry| entry.name == "plain-dir")
        .expect("plain entry exists");
    assert!(plain_entry.is_dir);
    assert!(!plain_entry.is_git_repo);
}

#[tokio::test]
async fn embedded_daemon_skips_noise_directories() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-skip");
    fs::create_dir_all(dir.path().join("node_modules")).expect("create node_modules");
    fs::create_dir_all(dir.path().join("target")).expect("create target");
    fs::create_dir_all(dir.path().join("src")).expect("create src");

    let response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            query_path(dir.path())
        ),
        StatusCode::OK,
    )
    .await;

    assert!(response.entries.iter().any(|entry| entry.name == "src"));
    assert!(!response
        .entries
        .iter()
        .any(|entry| entry.name == "node_modules"));
    assert!(!response.entries.iter().any(|entry| entry.name == "target"));
}

#[tokio::test]
async fn embedded_daemon_includes_files_and_directories() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-files");
    fs::write(dir.path().join("README.md"), "# Test\n").expect("write readme");
    fs::create_dir_all(dir.path().join("src")).expect("create src");

    let response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            query_path(dir.path())
        ),
        StatusCode::OK,
    )
    .await;

    let names: Vec<_> = response
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["src", "README.md"]);

    let src_entry = response
        .entries
        .iter()
        .find(|entry| entry.name == "src")
        .expect("src entry exists");
    assert!(src_entry.is_dir);

    let readme_entry = response
        .entries
        .iter()
        .find(|entry| entry.name == "README.md")
        .expect("readme entry exists");
    assert!(!readme_entry.is_dir);
    assert!(!readme_entry.is_git_repo);
}

#[tokio::test]
async fn embedded_daemon_filters_git_internal_dir() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-git-filter");
    let repo_path = dir.path().join("repo");
    fs::create_dir_all(&repo_path).expect("create repo dir");
    run_git(&repo_path, &["init"]);

    let repo_response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            query_path(&repo_path)
        ),
        StatusCode::OK,
    )
    .await;

    assert!(!repo_response
        .entries
        .iter()
        .any(|entry| entry.name == ".git"));

    let parent_response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            query_path(dir.path())
        ),
        StatusCode::OK,
    )
    .await;

    let repo_entry = parent_response
        .entries
        .iter()
        .find(|entry| entry.name == "repo")
        .expect("repo entry exists");
    assert!(repo_entry.is_dir);
    assert!(repo_entry.is_git_repo);
}

#[tokio::test]
async fn embedded_daemon_nonexistent_path_returns_client_error() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-missing");
    let missing = dir.path().join("missing");

    let response = raw_empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            query_path(&missing)
        ),
    )
    .await;

    assert!(
        matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
        ),
        "unexpected status: {}",
        response.status()
    );
}

#[tokio::test]
async fn embedded_daemon_resolves_home_shortcut() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let expected_home = PathBuf::from(std::env::var("HOME").expect("HOME set"))
        .canonicalize()
        .expect("home canonicalizes");

    let response: FsListResponse = empty_request(
        &app,
        &format!("/api/v1/fs/list?path=~&daemon_id={daemon_id}"),
        StatusCode::OK,
    )
    .await;

    assert_eq!(response.path, expected_home.to_string_lossy());
}

#[tokio::test]
async fn embedded_daemon_resolves_relative_path_from_launch_directory() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let cwd = std::env::current_dir().expect("current dir");
    let dir = TestDir::new_in(&cwd, "forge-api-fs-routing-relative");

    let response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?path={}&daemon_id={daemon_id}",
            dir.path().file_name().expect("dir name").to_string_lossy()
        ),
        StatusCode::OK,
    )
    .await;

    assert_eq!(
        response.path,
        dir.path()
            .canonicalize()
            .expect("dir canonical")
            .to_string_lossy()
    );
}

#[tokio::test]
async fn embedded_daemon_branches_returns_local_branches() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-branches");
    let repo_path = dir.path();
    run_git(repo_path, &["init", "-b", "main"]);
    run_git(repo_path, &["config", "user.email", "test@forge.dev"]);
    run_git(repo_path, &["config", "user.name", "Forge Test"]);
    fs::write(repo_path.join("README.md"), "# Test\n").expect("write readme");
    run_git(repo_path, &["add", "README.md"]);
    run_git(repo_path, &["commit", "-m", "initial"]);
    run_git(repo_path, &["branch", "feature/test"]);
    run_git(
        repo_path,
        &["remote", "add", "origin", "https://example.com/acme/fs.git"],
    );

    let response: BranchListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/branches?path={}&daemon_id={daemon_id}",
            query_path(repo_path)
        ),
        StatusCode::OK,
    )
    .await;

    assert!(response.branches.iter().any(|branch| branch == "main"));
    assert!(response
        .branches
        .iter()
        .any(|branch| branch == "feature/test"));
    assert_eq!(response.default_branch.as_deref(), Some("main"));
    assert_eq!(
        response.origin_url.as_deref(),
        Some("https://example.com/acme/fs.git")
    );
}

#[tokio::test]
async fn embedded_daemon_branches_non_git_dir_returns_not_git_error() {
    let state = test_state().await;
    let app = test_app(&state);
    let daemon_id = seed_embedded_daemon(&state).await;
    let dir = TestDir::new("forge-api-fs-routing-not-git");

    let response = raw_empty_request(
        &app,
        &format!(
            "/api/v1/fs/branches?path={}&daemon_id={daemon_id}",
            query_path(dir.path())
        ),
    )
    .await;
    let error: ErrorResponse = parse_response(response, StatusCode::BAD_REQUEST).await;
    assert_eq!(error.code, "fs.not_a_git_repo");
}

#[tokio::test]
async fn connected_remote_daemon_lists_via_command_socket() {
    let state = test_state().await;
    let app = test_app(&state);
    let registration = register_daemon(&app, "fs-routing-connected-remote").await;
    let (connection, outbound) =
        services::daemon_transport::DaemonConnection::new(registration.daemon_id.clone());
    state
        .daemon_connections
        .register(registration.daemon_id.clone(), connection);
    assert!(state
        .daemon_connections
        .is_connected(&registration.daemon_id));

    let remote_path = "/remote/project";
    let responder = tokio::spawn(respond_to_fs_list(
        state.daemon_connections.clone(),
        registration.daemon_id.clone(),
        outbound,
        remote_path.to_owned(),
    ));

    let response: FsListResponse = empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?daemon_id={}&path={remote_path}",
            registration.daemon_id
        ),
        StatusCode::OK,
    )
    .await;

    assert_eq!(response.path, remote_path);
    assert_eq!(response.entries.len(), 1);
    assert_eq!(response.entries[0].name, "remote.txt");
    assert_eq!(
        response.entries[0].path,
        "/remote/project/remote.txt".to_owned()
    );

    responder.await.expect("responder task joins");
}

#[tokio::test]
async fn disconnected_remote_daemon_returns_unavailable() {
    let state = test_state().await;
    let app = test_app(&state);
    let registration = register_daemon(&app, "fs-routing-disconnected-remote").await;
    let dir = TestDir::new("forge-api-fs-routing-disconnected");

    let response = raw_empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?daemon_id={}&path={}",
            registration.daemon_id,
            query_path(dir.path())
        ),
    )
    .await;
    let (status, body) = read_response(response).await;

    assert!(
        matches!(
            status,
            StatusCode::CONFLICT | StatusCode::SERVICE_UNAVAILABLE
        ),
        "unexpected status {status} with body {}",
        String::from_utf8_lossy(&body)
    );
    let error: ErrorResponse = serde_json::from_slice(&body).expect("parse error response");
    assert_eq!(error.code, "daemon_unavailable");
}

#[tokio::test]
async fn disconnected_filesystem_request_is_not_queued_for_report_polling() {
    let state = test_state().await;
    let app = test_app(&state);
    let registration = register_daemon(&app, "fs-routing-no-polling").await;
    let dir = TestDir::new("forge-api-fs-routing-no-polling");

    let failed = raw_empty_request(
        &app,
        &format!(
            "/api/v1/fs/list?daemon_id={}&path={}",
            registration.daemon_id,
            query_path(dir.path())
        ),
    )
    .await;
    let (status, body) = read_response(failed).await;
    assert!(
        matches!(
            status,
            StatusCode::CONFLICT | StatusCode::SERVICE_UNAVAILABLE
        ),
        "unexpected status {status} with body {}",
        String::from_utf8_lossy(&body)
    );
    let error: ErrorResponse = serde_json::from_slice(&body).expect("parse error response");
    assert_eq!(error.code, "daemon_unavailable");

    let report_body = json_request_with_bearer_text(
        &app,
        Method::POST,
        &format!("/api/v1/daemons/{}/report", registration.daemon_id),
        &registration.registration_token,
        json!({
            "detected_clis": [],
            "runtimes": [{
                "kind": "local",
                "workspace_root": "/remote/workspaces",
                "status": "ready"
            }],
            "labels": { "suite": "fs_daemon_routing" }
        }),
        StatusCode::OK,
    )
    .await;

    assert!(!report_body.contains("pending_commands"));
    assert!(!report_body.contains(METHOD_FS_LIST));
}

#[tokio::test]
async fn execution_terminal_notification_from_non_owner_daemon_is_rejected() {
    let state = test_state().await;
    let app = test_app(&state);
    let owner_registration = register_daemon(&app, "execution-owner-daemon").await;
    let other_registration = register_daemon(&app, "execution-other-daemon").await;
    let server = TestServer::start(Arc::clone(&state)).await;

    let _owner_socket = connect_daemon(
        &server,
        &owner_registration.daemon_id,
        &owner_registration.registration_token,
    )
    .await
    .expect("owner websocket upgrade succeeds");
    let mut other_socket = connect_daemon(
        &server,
        &other_registration.daemon_id,
        &other_registration.registration_token,
    )
    .await
    .expect("other websocket upgrade succeeds");
    wait_until_connected(&state, &owner_registration.daemon_id).await;
    wait_until_connected(&state, &other_registration.daemon_id).await;

    let execution = seed_running_execution_for_daemon(&state, &owner_registration.daemon_id).await;

    let terminal = DaemonFrame::Notification {
        method: METHOD_EXECUTION_TERMINAL.to_owned(),
        params: serde_json::to_value(ExecutionTerminalNotification {
            execution_id: execution.id.clone(),
            exit_code: Some(0),
            signal: None,
            error: None,
            ts: db::now_rfc3339(),
        })
        .expect("terminal notification serializes"),
    };
    other_socket
        .send(WsMessage::Text(
            serde_json::to_string(&terminal).expect("daemon frame serializes"),
        ))
        .await
        .expect("send forged terminal notification");
    assert_heartbeat_echo(&mut other_socket, 42).await;

    assert_execution_status_remains(&state, &execution.id, ExecutionStatus::Running).await;
}

async fn respond_to_fs_list(
    registry: Arc<services::daemon_transport::DaemonConnectionRegistry>,
    daemon_id: String,
    mut outbound: tokio::sync::mpsc::Receiver<DaemonFrame>,
    expected_path: String,
) {
    let frame = outbound.recv().await.expect("server sends request frame");
    let DaemonFrame::Request { id, method, params } = frame else {
        panic!("expected daemon request frame");
    };

    assert_eq!(method, METHOD_FS_LIST);
    assert_eq!(params["path"], expected_path);

    let result = serde_json::to_value(FsListResult {
        path: expected_path.clone(),
        entries: vec![FsEntry {
            name: "remote.txt".to_owned(),
            path: format!("{expected_path}/remote.txt"),
            is_dir: false,
            is_git_repo: false,
        }],
    })
    .expect("serialize fs list result");
    registry.dispatch_incoming(&daemon_id, DaemonFrame::Response { id, result });
}

async fn test_state() -> Arc<AppState> {
    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    db::run_migrations(&pool).await.expect("migrations run");

    let db = Arc::new(db::SqliteDb::new(pool));
    seed_test_user(&db).await;
    let event_bus = Arc::new(events::EventBus::new(16));
    Arc::new(AppState::new(db, event_bus, true))
}

fn test_app(state: &AppState) -> Router {
    build_router(state.clone(), temp_web_dist())
}

async fn seed_test_user(db: &db::SqliteDb) {
    let now = db::now_rfc3339();
    UserRepo::create_user(
        db,
        &db::User {
            id: TEST_USER_ID.to_owned(),
            email: "test@example.com".to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: true,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("seed test user");
}

async fn seed_embedded_daemon(state: &AppState) -> String {
    let now = db::now_rfc3339();
    let daemon = DaemonRepo::upsert_by_machine_id(
        &*state.db,
        UpsertDaemon {
            id: uuid::Uuid::new_v4().to_string(),
            machine_id: services::embedded_daemon::embedded_machine_id(),
            hostname: "embedded-test-host".to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            agent_version: None,
            labels_json: r#"{"mode":"embedded"}"#.to_owned(),
            status: DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: Some(TEST_USER_ID.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("seed embedded daemon");
    daemon.id
}

async fn register_daemon(app: &Router, machine_id: &str) -> DaemonRegisterResponse {
    json_request(
        app,
        Method::POST,
        "/api/v1/daemons/register",
        json!({
            "machine_id": machine_id,
            "hostname": "remote-test-host",
            "os": "linux",
            "arch": "x86_64",
            "agent_version": "fs-routing-test",
            "labels": { "suite": "fs_daemon_routing" }
        }),
        StatusCode::OK,
    )
    .await
}

async fn connect_daemon(
    server: &TestServer,
    daemon_id: &str,
    token: &str,
) -> Result<ClientSocket, WsError> {
    let url = format!("ws://{}/api/v1/daemons/{daemon_id}/connect", server.addr);
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        header::AUTHORIZATION.as_str(),
        HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
    );

    connect_async(request).await.map(|(socket, _)| socket)
}

async fn wait_until_connected(state: &AppState, daemon_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if state.daemon_connections.is_connected(daemon_id) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon connection was not registered"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn assert_heartbeat_echo(socket: &mut ClientSocket, seq: u64) {
    let heartbeat = DaemonFrame::Heartbeat { seq };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&heartbeat).expect("heartbeat serializes"),
        ))
        .await
        .expect("send heartbeat");
    let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("heartbeat response arrives")
        .expect("websocket remains open")
        .expect("heartbeat response is valid");
    let WsMessage::Text(text) = message else {
        panic!("expected heartbeat text frame, got {message:?}");
    };
    let frame: DaemonFrame = serde_json::from_str(&text).expect("heartbeat frame parses");
    assert!(matches!(frame, DaemonFrame::Heartbeat { seq: received } if received == seq));
}

async fn seed_running_execution_for_daemon(state: &AppState, daemon_id: &str) -> Execution {
    let now = db::now_rfc3339();
    let project_id = uuid::Uuid::new_v4().to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    let agent_id = uuid::Uuid::new_v4().to_string();

    ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: project_id.clone(),
            name: "Execution Ownership".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(TEST_USER_ID.to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    TaskRepo::create(
        &*state.db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: None,
            parent_task_id: None,
            assignee_type: Some("agent".to_owned()),
            assignee_id: Some(agent_id.clone()),
            title: "Owned execution".to_owned(),
            description: None,
            task_type: "task".to_owned(),
            status: "in_progress".to_owned(),
            priority: 0,
            subtask_order: None,
            task_state_config: None,
            merge_config: None,
            plan: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    AgentRepo::create(
        &*state.db,
        CreateAgent {
            id: agent_id.clone(),
            name: "Owner Agent".to_owned(),
            description: None,
            executor_type: "codex".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            daemon_id: Some(daemon_id.to_owned()),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Busy,
            last_heartbeat_at: Some(now.clone()),
            is_default: false,
            paused: false,
            owner_id: Some(TEST_USER_ID.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("agent creates");
    ExecutionRepo::create(
        &*state.db,
        CreateExecution {
            id: uuid::Uuid::new_v4().to_string(),
            task_id,
            agent_id: Some(agent_id),
            role: "coder".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: Some(now.clone()),
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("execution creates")
}

async fn assert_execution_status_remains(
    state: &AppState,
    execution_id: &str,
    expected_status: ExecutionStatus,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
    loop {
        let execution = ExecutionRepo::get_by_id(&*state.db, execution_id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(
            execution.status, expected_status,
            "forged daemon terminal notification changed execution status"
        );
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn empty_request<T: DeserializeOwned>(
    app: &Router,
    uri: &str,
    expected_status: StatusCode,
) -> T {
    let response = raw_empty_request(app, uri).await;
    parse_response(response, expected_status).await
}

async fn raw_empty_request(app: &Router, uri: &str) -> axum::response::Response {
    let token = test_jwt();
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response")
}

async fn json_request<T>(
    app: &Router,
    method: Method,
    uri: &str,
    body: Value,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    let response = raw_json_request(app, method, uri, None, body).await;
    parse_response(response, expected_status).await
}

async fn json_request_with_bearer_text(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
    expected_status: StatusCode,
) -> String {
    let response = raw_json_request(app, method, uri, Some(token), body).await;
    let (status, bytes) = read_response(response).await;
    assert_eq!(
        status,
        expected_status,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    String::from_utf8(bytes).expect("response is utf8")
}

async fn raw_json_request(
    app: &Router,
    method: Method,
    uri: &str,
    bearer: Option<&str>,
    body: Value,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    app.clone()
        .oneshot(
            builder
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .expect("build JSON request"),
        )
        .await
        .expect("router response")
}

async fn parse_response<T>(response: axum::response::Response, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let (status, bytes) = read_response(response).await;
    assert_eq!(
        status,
        expected_status,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse response body")
}

async fn read_response(response: axum::response::Response) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body")
        .to_vec();
    (status, bytes)
}

fn test_jwt() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": TEST_USER_ID,
        "email": "test@example.com",
        "is_admin": true,
        "iat": now,
        "exp": now + 900,
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"test-jwt-secret-for-development"),
    )
    .expect("encode test jwt")
}

fn query_path(path: &Path) -> String {
    path.to_string_lossy().replace(' ', "%20")
}

fn run_git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temp_web_dist() -> PathBuf {
    let path = std::env::temp_dir().join(format!("forge-api-fs-routing-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&path).expect("create temp web dist");
    fs::write(path.join("index.html"), "<html></html>").expect("write index");
    path
}

struct TestServer {
    addr: std::net::SocketAddr,
    state: Arc<AppState>,
    handle: tokio::task::JoinHandle<()>,
    _web_dist_dir: TestDir,
}

impl TestServer {
    async fn start(state: Arc<AppState>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let web_dist_dir = TestDir::new("forge-api-fs-routing-web");
        fs::write(web_dist_dir.path().join("index.html"), "<html></html>").expect("write index");
        let web_dist_path = web_dist_dir.path().to_path_buf();
        let server_state = (*state).clone();
        let shutdown_signal = state.shutdown_signal.clone();

        let handle = tokio::spawn(async move {
            serve_with_listener(listener, server_state, web_dist_path, async move {
                shutdown_signal.wait().await;
            })
            .await
            .expect("test API server serves");
        });

        Self {
            addr,
            state,
            handle,
            _web_dist_dir: web_dist_dir,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.state.shutdown_signal.request();
        self.handle.abort();
    }
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        Self::new_in(&std::env::temp_dir(), prefix)
    }

    fn new_in(root: &Path, prefix: &str) -> Self {
        let path = root.join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
