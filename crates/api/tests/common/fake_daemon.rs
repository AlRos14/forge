#![allow(dead_code)]

use std::{sync::Arc, time::Duration};

use api::{serve_with_listener, AppState};
use api_types::{
    DaemonFrame, DaemonRegisterResponse, DaemonResponse, ExecutionLogNotification,
    ExecutionStartResult, ExecutionTerminalNotification, FsEntry, FsListResult,
    TerminalClientFrame, TerminalServerFrame, DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1,
    DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1, METHOD_EXECUTION_LOG,
    METHOD_EXECUTION_START, METHOD_EXECUTION_TERMINAL, METHOD_FS_LIST,
    METHOD_PROTOCOL_CAPABILITIES,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use db::{
    AgentRepo, AgentStatus, CreateAgent, CreateDomainEvent, CreateExecution, CreateProject,
    CreateRepo, CreateRoleMembership, CreateTask, CreateTaskRole, CreateWorkUnit,
    CreateWorkUnitExecution, CreateWorkUnitWorkspace, CreateWorkspace, CreateWorkspaceLease,
    DaemonRepo, DaemonStatus, Execution, ExecutionRepo, ExecutionStatus, ProjectRepo, RepoRepo,
    RoleMembershipRepo, RoleMembershipStatus, TaskRepo, TaskRoleRepo, UpdateDaemonReport,
    UpsertDaemon, UserRepo, WorkMode, WorkUnit, WorkUnitExecutionRepo, WorkUnitRepo,
    WorkUnitWorkspaceRepo, WorkspaceRepo, WorkspaceStatus,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest, http::HeaderValue, Error as WsError, Message as WsMessage,
    },
    MaybeTlsStream, WebSocketStream,
};
use tower::ServiceExt;

use super::{json_request, json_request_with_bearer, TestDir};

pub const FAKE_DAEMON_USER_ID: &str = "test-user-id";

fn test_domain_event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    actor_type: &str,
    actor_id: &str,
    payload: Value,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: actor_type.to_owned(),
        actor_id: Some(actor_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: payload.to_string(),
        created_at: db::now_rfc3339(),
    }
}

fn provider_start_attempt_event(execution: &Execution) -> CreateDomainEvent {
    let agent_id = execution
        .agent_id
        .as_deref()
        .expect("running fixture Execution has an Agent");
    CreateDomainEvent {
        id: uuid::Uuid::new_v4().to_string(),
        event_type: "execution.provider_start_attempted".to_owned(),
        entity_type: "execution".to_owned(),
        entity_id: execution.id.clone(),
        actor_type: "agent".to_owned(),
        actor_id: Some(agent_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: execution.task_id.clone(),
        correlation_id: execution.id.clone(),
        causation_id: execution.parent_execution_id.clone(),
        causation_depth: i64::from(execution.parent_execution_id.is_some()),
        dedupe_key: Some(format!(
            "execution.provider_start_attempted:{}",
            execution.id
        )),
        payload_json: json!({
            "execution_id": execution.id,
            "task_id": execution.task_id,
            "agent_id": agent_id,
            "outcome": "attempted"
        })
        .to_string(),
        created_at: db::now_rfc3339(),
    }
}

async fn create_implementation_work_unit(
    state: &AppState,
    task: &db::Task,
    agent_id: &str,
    requires_integration: bool,
) -> WorkUnit {
    let now = db::now_rfc3339();
    let role = TaskRoleRepo::create(
        &*state.db,
        CreateTaskRole {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task.id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("implementer TaskRole creates");
    RoleMembershipRepo::add(
        &*state.db,
        CreateRoleMembership {
            id: uuid::Uuid::new_v4().to_string(),
            task_role_id: role.id,
            actor_kind: db::ActorKind::Agent,
            actor_id: agent_id.to_owned(),
            status: RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent joins implementer TaskRole");

    let id = uuid::Uuid::new_v4().to_string();
    WorkUnitRepo::create(
        &*state.db,
        CreateWorkUnit {
            id: id.clone(),
            task_id: task.id.clone(),
            parent_work_unit_id: None,
            title: "Daemon routing WorkUnit".to_owned(),
            scope: "Exercise the daemon execution protocol".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(db::ActorRef::Agent(agent_id.to_owned())),
            requires_integration,
            provenance: None,
            created_by: db::ActorRef::Human(FAKE_DAEMON_USER_ID.to_owned()),
            created_at: now,
        },
        test_domain_event(
            "work_unit.created",
            "work_unit",
            &id,
            &task.id,
            "human",
            FAKE_DAEMON_USER_ID,
            json!({"task_id": task.id, "work_unit_id": id, "version": 1, "status": "open"}),
        ),
    )
    .await
    .expect("WorkUnit creates")
    .record
}

async fn authorized_daemon_execution_snapshot(
    state: &AppState,
    agent_id: &str,
    executor_type: &str,
) -> Value {
    let agent = AgentRepo::get_by_id(&*state.db, agent_id)
        .await
        .expect("execution fixture Agent loads")
        .expect("execution fixture Agent exists");
    let daemon_id = agent
        .daemon_id
        .as_deref()
        .expect("execution fixture Agent is assigned to a daemon");
    let now = db::now_rfc3339();
    DaemonRepo::update_report(
        &*state.db,
        UpdateDaemonReport {
            id: daemon_id.to_owned(),
            last_report_at: now.clone(),
            status: DaemonStatus::Online,
            detected_clis_json: json!([{
                "kind": executor_type,
                "availability": "authenticated"
            }])
            .to_string(),
            labels_json: None,
            updated_at: now,
        },
    )
    .await
    .expect("execution fixture daemon reports authenticated executor");
    let daemon = DaemonRepo::get_by_id(&*state.db, daemon_id)
        .await
        .expect("execution fixture daemon loads")
        .expect("execution fixture daemon exists");

    json!({
        "agent_id": agent.id,
        "agent_daemon_id": agent.daemon_id,
        "daemon_authority": db::daemon_authority_snapshot(&agent, &daemon),
        "executor_type": executor_type,
        "config": {},
        "resolved_daemon_id": daemon.id,
    })
}

pub type ClientSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub struct TestServer {
    pub addr: std::net::SocketAddr,
    pub state: Arc<AppState>,
    handle: tokio::task::JoinHandle<()>,
    _web_dist_dir: TestDir,
}

impl TestServer {
    pub async fn start(state: Arc<AppState>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let web_dist_dir = TestDir::new("forge-api-fake-daemon-web");
        std::fs::write(web_dist_dir.path().join("index.html"), "<html></html>")
            .expect("write index");
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

pub async fn register_daemon(
    app: &Router,
    machine_id: &str,
    suite: &str,
) -> DaemonRegisterResponse {
    json_request(
        app,
        Method::POST,
        "/api/v1/daemons/register",
        json!({
            "machine_id": machine_id,
            "hostname": "remote-test-host",
            "os": "linux",
            "arch": "x86_64",
            "agent_version": "fake-daemon-test",
            "labels": { "suite": suite }
        }),
        StatusCode::OK,
    )
    .await
}

pub async fn report_remote_daemon_shell(
    app: &Router,
    daemon_id: &str,
    registration_token: &str,
    workspace_root: &std::path::Path,
    suite: &str,
) {
    let _: DaemonResponse = json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/daemons/{daemon_id}/report"),
        registration_token,
        json!({
            "detected_clis": [{
                "kind": "shell",
                "availability": "authenticated",
                "path": "/bin/sh"
            }],
            "runtimes": [{
                "kind": "local",
                "workspace_root": workspace_root.to_string_lossy(),
                "status": "ready"
            }],
            "labels": { "suite": suite }
        }),
        StatusCode::OK,
    )
    .await;
}

pub async fn connect_daemon(
    server: &TestServer,
    daemon_id: &str,
    token: Option<&str>,
) -> Result<ClientSocket, WsError> {
    let url = format!("ws://{}/api/v1/daemons/{daemon_id}/connect", server.addr);
    let request = if let Some(token) = token {
        let mut request = url.into_client_request()?;
        request.headers_mut().insert(
            header::AUTHORIZATION.as_str(),
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );
        request
    } else {
        url.into_client_request()?
    };

    connect_async(request).await.map(|(socket, _)| socket)
}

pub async fn connect_terminal(
    server: &TestServer,
    session_id: &str,
    attach_token: &str,
) -> Result<ClientSocket, WsError> {
    let url = format!(
        "ws://{}/api/v1/terminals/{session_id}/ws?attach_token={attach_token}",
        server.addr
    );
    connect_async(url).await.map(|(socket, _)| socket)
}

pub async fn next_daemon_request(
    socket: &mut ClientSocket,
    expected_method: &str,
) -> (String, Value) {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("daemon request arrives")
            .expect("daemon websocket remains open")
            .expect("daemon request is valid");
        let WsMessage::Text(text) = message else {
            continue;
        };
        let frame: DaemonFrame = serde_json::from_str(text.as_ref()).expect("daemon frame parses");
        match frame {
            DaemonFrame::Request { id, method, params } => {
                if method == METHOD_PROTOCOL_CAPABILITIES {
                    send_daemon_response(
                        socket,
                        id,
                        api_types::DaemonProtocolCapabilities {
                            schema_version: 1,
                            features: vec![
                                DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1.to_owned(),
                                DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1.to_owned(),
                            ],
                        },
                    )
                    .await;
                    continue;
                }
                assert_eq!(method, expected_method);
                return (id, params);
            }
            DaemonFrame::Heartbeat { .. } => {}
            other => panic!("expected daemon request frame, got {other:?}"),
        }
    }
}

pub async fn send_daemon_response<T: Serialize>(socket: &mut ClientSocket, id: String, result: T) {
    let frame = DaemonFrame::Response {
        id,
        result: serde_json::to_value(result).expect("daemon result serializes"),
    };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&frame)
                .expect("daemon response serializes")
                .into(),
        ))
        .await
        .expect("send daemon response");
}

pub async fn send_daemon_notification<T: Serialize>(
    socket: &mut ClientSocket,
    method: &str,
    params: T,
) {
    let frame = DaemonFrame::Notification {
        method: method.to_owned(),
        params: serde_json::to_value(params).expect("daemon notification serializes"),
    };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&frame)
                .expect("daemon notification frame serializes")
                .into(),
        ))
        .await
        .expect("send daemon notification");
}

pub async fn send_terminal_client_frame(socket: &mut ClientSocket, frame: TerminalClientFrame) {
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&frame)
                .expect("terminal client frame serializes")
                .into(),
        ))
        .await
        .expect("send terminal client frame");
}

pub async fn next_terminal_server_frame(socket: &mut ClientSocket) -> TerminalServerFrame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("terminal server frame arrives")
            .expect("terminal websocket remains open")
            .expect("terminal frame is valid");
        let WsMessage::Text(text) = message else {
            continue;
        };
        return serde_json::from_str(text.as_ref()).expect("terminal server frame parses");
    }
}

pub async fn wait_until_connected(state: &AppState, daemon_id: &str) {
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

pub async fn wait_until_disconnected(state: &AppState, daemon_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if !state.daemon_connections.is_connected(daemon_id) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon connection remained registered"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn assert_heartbeat_echo(socket: &mut ClientSocket, seq: u64) {
    let heartbeat = DaemonFrame::Heartbeat { seq };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&heartbeat)
                .expect("heartbeat serializes")
                .into(),
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

pub async fn respond_to_fs_list(
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

pub async fn accept_execution_start(
    socket: &mut ClientSocket,
    execution_id: &str,
) -> api_types::ExecutionStartParams {
    let (start_id, params) = next_daemon_request(socket, METHOD_EXECUTION_START).await;
    let start_params: api_types::ExecutionStartParams =
        serde_json::from_value(params).expect("execution.start params parse");
    assert_eq!(start_params.execution_id, execution_id);
    send_daemon_response(
        socket,
        start_id,
        ExecutionStartResult {
            execution_id: execution_id.to_owned(),
            accepted: true,
        },
    )
    .await;
    start_params
}

pub async fn send_execution_log(
    socket: &mut ClientSocket,
    execution_id: &str,
    seq: u64,
    line: &str,
) {
    send_daemon_notification(
        socket,
        METHOD_EXECUTION_LOG,
        ExecutionLogNotification {
            execution_id: execution_id.to_owned(),
            seq,
            stream: "stdout".to_owned(),
            line: line.to_owned(),
            ts: db::now_rfc3339(),
            kind: Some("stdout".to_owned()),
            log_stream: Some("main".to_owned()),
            payload: Some(json!({ "line": line })),
            truncated: Some(false),
        },
    )
    .await;
}

pub async fn send_execution_terminal_completed(
    socket: &mut ClientSocket,
    execution_id: &str,
    summary: Option<&str>,
) {
    send_daemon_notification(
        socket,
        METHOD_EXECUTION_TERMINAL,
        ExecutionTerminalNotification {
            execution_id: execution_id.to_owned(),
            exit_code: Some(0),
            signal: None,
            error: None,
            ts: db::now_rfc3339(),
            status: Some("completed".to_owned()),
            agent_session_id: None,
            summary: summary.map(str::to_owned),
            assistant_output: None,
            after_sha: None,
            usage: None,
            account_usage: None,
            failure_class: None,
            retry_at: None,
            resolved_candidate: None,
            route_attempts: None,
        },
    )
    .await;
}

pub async fn send_execution_terminal_failed(
    socket: &mut ClientSocket,
    execution_id: &str,
    error: &str,
) {
    send_daemon_notification(
        socket,
        METHOD_EXECUTION_TERMINAL,
        ExecutionTerminalNotification {
            execution_id: execution_id.to_owned(),
            exit_code: Some(1),
            signal: None,
            error: Some(error.to_owned()),
            ts: db::now_rfc3339(),
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
    )
    .await;
}

pub async fn seed_running_execution_for_daemon(state: &AppState, daemon_id: &str) -> Execution {
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
            primary_repo_id: None,
            owner_id: Some(FAKE_DAEMON_USER_ID.to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    let task = TaskRepo::create(
        &*state.db,
        CreateTask {
            id: task_id.clone(),
            project_id: project_id.clone(),
            repo_id: None,
            parent_task_id: None,
            title: "Owned execution".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Active,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
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
            credential_ref: None,
            daemon_id: Some(daemon_id.to_owned()),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Busy,
            last_heartbeat_at: Some(now.clone()),
            is_default: false,
            paused: false,
            owner_id: Some(FAKE_DAEMON_USER_ID.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("agent creates");

    let executor_snapshot = authorized_daemon_execution_snapshot(state, &agent_id, "codex").await;

    let work_unit = create_implementation_work_unit(state, &task, &agent_id, false).await;
    let execution = CreateExecution {
        id: uuid::Uuid::new_v4().to_string(),
        task_id: task_id.clone(),
        agent_id: Some(agent_id.clone()),
        actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
        purpose: Some(db::ExecutionPurpose::Implement),
        harness_session_id: None,
        role: "implementer".to_owned(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: None,
        agent_message_id: None,
        last_activity_at: Some(now.clone()),
        summary: None,
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: Some(
            json!({
                "agent_id": executor_snapshot["agent_id"],
                "agent_daemon_id": executor_snapshot["agent_daemon_id"],
                "daemon_authority": executor_snapshot["daemon_authority"],
                "executor_type": executor_snapshot["executor_type"],
                "config": executor_snapshot["config"],
                "resolved_daemon_id": executor_snapshot["resolved_daemon_id"],
                "harness_capabilities": api_types::HarnessCapabilities::unknown().snapshot()
            })
            .to_string(),
        ),
        workspace_id: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let execution_id = execution.id.clone();
    let execution = WorkUnitExecutionRepo::create_for_work_unit(
        &*state.db,
        CreateWorkUnitExecution {
            execution,
            work_unit_id: work_unit.id.clone(),
            work_unit_version: work_unit.version,
            workspace_lease: None,
        },
        test_domain_event(
            "execution.started",
            "execution",
            &execution_id,
            &task_id,
            "agent",
            &agent_id,
            json!({
                "task_id": task_id,
                "work_unit_id": work_unit.id,
                "execution_id": execution_id,
                "work_unit_version": work_unit.version
            }),
        ),
    )
    .await
    .expect("WorkUnit Execution creates")
    .record;
    let attempt = ExecutionRepo::claim_provider_start(
        &*state.db,
        &execution.id,
        provider_start_attempt_event(&execution),
    )
    .await
    .expect("provider Start attempt receipt creates");
    assert_eq!(attempt, db::ProviderStartClaim::Claimed);
    execution
}

pub struct StartableExecutionFixture {
    pub task_id: String,
    pub workspace_path: String,
    pub execution: Execution,
}

pub async fn seed_startable_execution_for_daemon(
    state: &AppState,
    daemon_id: &str,
    prompt: &str,
) -> StartableExecutionFixture {
    let now = db::now_rfc3339();
    let project_id = uuid::Uuid::new_v4().to_string();
    let repo_id = uuid::Uuid::new_v4().to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    let agent_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();
    let workspace_path = state
        .cleanup_scheduler
        .workspace_root()
        .join("execution-start")
        .join(&task_id);
    std::fs::create_dir_all(&workspace_path).expect("workspace path creates");

    ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: project_id.clone(),
            name: "Execution Start".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(FAKE_DAEMON_USER_ID.to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    RepoRepo::create(
        &*state.db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "execution-repo".to_owned(),
            remote_url: "file:///tmp/execution-repo".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo creates");
    let task = TaskRepo::create(
        &*state.db,
        CreateTask {
            id: task_id.clone(),
            project_id: project_id.clone(),
            repo_id: Some(repo_id.clone()),
            parent_task_id: None,
            title: "Start remote execution".to_owned(),
            description: Some(prompt.to_owned()),
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Active,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
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
            name: "Remote Shell".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id.to_owned()),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Busy,
            last_heartbeat_at: Some(now.clone()),
            is_default: false,
            paused: false,
            owner_id: Some(FAKE_DAEMON_USER_ID.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("agent creates");
    let executor_snapshot = authorized_daemon_execution_snapshot(state, &agent_id, "shell").await;
    let work_unit = create_implementation_work_unit(state, &task, &agent_id, true).await;
    let workspace = WorkUnitWorkspaceRepo::create_for_work_unit(
        &*state.db,
        CreateWorkUnitWorkspace {
            workspace: CreateWorkspace {
                id: workspace_id.clone(),
                task_id: task_id.clone(),
                repo_id: repo_id.clone(),
                worktree_path: workspace_path.to_string_lossy().into_owned(),
                branch: "forge/work-unit/start".to_owned(),
                status: WorkspaceStatus::Ready,
                before_sha: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            work_unit_id: work_unit.id.clone(),
        },
    )
    .await
    .expect("WorkUnit workspace creates");
    let execution_input = CreateExecution {
        id: uuid::Uuid::new_v4().to_string(),
        task_id: task_id.clone(),
        agent_id: Some(agent_id.clone()),
        actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
        purpose: Some(db::ExecutionPurpose::Implement),
        harness_session_id: None,
        role: "implementer".to_owned(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: None,
        agent_message_id: None,
        last_activity_at: Some(now.clone()),
        summary: None,
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: Some(executor_snapshot.to_string()),
        workspace_id: Some(workspace.id.clone()),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let execution_id = execution_input.id.clone();
    let capability_profile_revision = "forge.capability-profile/v1";
    let capability_class = "repository_write";
    let mut capability_digest = Sha256::new();
    capability_digest.update(capability_profile_revision.as_bytes());
    capability_digest.update([0]);
    capability_digest.update(capability_class.as_bytes());
    let execution = WorkUnitExecutionRepo::create_for_work_unit(
        &*state.db,
        CreateWorkUnitExecution {
            execution: execution_input,
            work_unit_id: work_unit.id.clone(),
            work_unit_version: work_unit.version,
            workspace_lease: Some(CreateWorkspaceLease {
                id: uuid::Uuid::new_v4().to_string(),
                project_id,
                task_id: task_id.clone(),
                work_unit_id: Some(work_unit.id.clone()),
                workspace_id: Some(workspace.id.clone()),
                task_version: task.version,
                execution_id: execution_id.clone(),
                operation_idempotency_key: execution_id.clone(),
                repository_binding_id: repo_id,
                base_ref: "main".to_owned(),
                role: "worker".to_owned(),
                capabilities_json: serde_json::to_string(&[capability_class])
                    .expect("lease capabilities serialize"),
                assigned_principal_type: "agent".to_owned(),
                assigned_principal_id: agent_id.clone(),
                capability_profile_revision: capability_profile_revision.to_owned(),
                capability_profile_digest: format!(
                    "sha256:{}",
                    hex::encode(capability_digest.finalize())
                ),
                issuing_principal_type: "system".to_owned(),
                issuing_principal_id: "task-service-scheduler".to_owned(),
                issued_at: now.clone(),
                expires_at: "2099-01-01T00:00:00+00:00".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            }),
        },
        test_domain_event(
            "execution.started",
            "execution",
            &execution_id,
            &task_id,
            "agent",
            &agent_id,
            json!({
                "task_id": task_id,
                "work_unit_id": work_unit.id,
                "execution_id": execution_id,
                "work_unit_version": work_unit.version
            }),
        ),
    )
    .await
    .expect("WorkUnit Execution and WorkspaceLease create")
    .record;

    StartableExecutionFixture {
        task_id,
        workspace_path: workspace_path.to_string_lossy().into_owned(),
        execution,
    }
}

pub struct TerminalTaskFixture {
    pub task_id: String,
    pub workspace_id: String,
}

pub async fn seed_local_terminal_task(state: &AppState) -> TerminalTaskFixture {
    let now = db::now_rfc3339();
    let project_id = uuid::Uuid::new_v4().to_string();
    let repo_id = uuid::Uuid::new_v4().to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();

    ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: project_id.clone(),
            name: "Local Terminal".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(FAKE_DAEMON_USER_ID.to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    RepoRepo::create(
        &*state.db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "terminal-repo".to_owned(),
            remote_url: "file:///tmp/terminal-repo".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo creates");
    TaskRepo::create(
        &*state.db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: Some(repo_id.clone()),
            parent_task_id: None,
            title: "Local terminal".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Active,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    let workspace_root = state.cleanup_scheduler.workspace_root().to_path_buf();
    std::fs::create_dir_all(&workspace_root).expect("workspace root exists");
    let worktree_path = workspace_root
        .join("local-terminal-routing")
        .join(&task_id)
        .join("repo");
    std::fs::create_dir_all(&worktree_path).expect("worktree path exists");
    WorkspaceRepo::create(
        &*state.db,
        CreateWorkspace {
            id: workspace_id.clone(),
            task_id: task_id.clone(),
            repo_id,
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: workspace::task_branch_name(&task_id),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("workspace creates");

    TerminalTaskFixture {
        task_id,
        workspace_id,
    }
}

pub async fn assert_execution_status_remains(
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

pub async fn wait_for_execution_status(
    state: &AppState,
    execution_id: &str,
    expected_status: ExecutionStatus,
) -> Execution {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let execution = ExecutionRepo::get_by_id(&*state.db, execution_id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        if execution.status == expected_status {
            return execution;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "execution did not reach {expected_status}, current status was {}",
            execution.status
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn poll_until_execution_status(
    state: &AppState,
    execution_id: &str,
    expected_status: ExecutionStatus,
) -> Execution {
    for _ in 0..100 {
        if let Some(execution) = ExecutionRepo::get_by_id(&*state.db, execution_id)
            .await
            .expect("execution lookup")
        {
            if execution.status == expected_status {
                return execution;
            }
            if expected_status == ExecutionStatus::Running
                && execution.status != ExecutionStatus::Running
            {
                panic!(
                    "execution left running early with status {:?}",
                    execution.status
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("execution {execution_id} did not reach {expected_status:?}");
}

pub async fn seed_embedded_daemon(state: &AppState) -> String {
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
            owner_id: Some(FAKE_DAEMON_USER_ID.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("seed embedded daemon");
    daemon.id
}

pub async fn seed_test_user(db: &db::SqliteDb) {
    let now = db::now_rfc3339();
    UserRepo::create_user(
        db,
        &db::User {
            id: FAKE_DAEMON_USER_ID.to_owned(),
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

pub async fn fetch_execution_logs(app: &Router, execution_id: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/executions/{execution_id}/logs?tail=50"))
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", super::test_jwt()),
                )
                .body(Body::empty())
                .expect("build logs request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected logs response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse logs response")
}
