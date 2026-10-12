#![allow(dead_code)]

mod common;

use std::{sync::Arc, time::Duration};

use api_types::{AgentResponse, ExecutionResponse, TaskResponse, METHOD_EXECUTION_START};
use axum::http::{Method, StatusCode};
use db::{
    ExecutionRepo, ExecutionStatus as DbExecutionStatus, StopReason, TaskLifecycleRepo,
    TaskLifecycleState, TaskRepo,
};
use futures_util::SinkExt;
use serde_json::{json, Value};
use services::HeartbeatMonitor;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use common::{
    admin_jwt, create_project_and_repo,
    fake_daemon::{
        connect_daemon, fetch_execution_logs, next_daemon_request, poll_until_execution_status,
        register_daemon, report_remote_daemon_shell, send_daemon_response, send_execution_log,
        send_execution_terminal_completed, send_execution_terminal_failed,
        wait_for_execution_status, wait_until_connected, wait_until_disconnected, TestServer,
    },
    json_request, json_request_with_bearer, setup_git_repo, test_app, TestDir,
};

async fn poll_task_lifecycle(
    db: &Arc<db::SqliteDb>,
    task_id: &str,
    expected: TaskLifecycleState,
) -> db::TaskLifecycle {
    for _ in 0..200 {
        if let Some(lifecycle) = TaskLifecycleRepo::get_task_lifecycle(&**db, task_id)
            .await
            .expect("task lookup")
        {
            if lifecycle.state == expected {
                return lifecycle;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&**db, task_id)
        .await
        .expect("task lookup")
        .expect("task lifecycle exists");
    panic!("task {task_id} did not reach {expected:?}; lifecycle={lifecycle:?}");
}

struct RemoteRoundtripFixture {
    harness: common::Harness,
    registration: api_types::DaemonRegisterResponse,
    server: TestServer,
    daemon_socket: common::fake_daemon::ClientSocket,
    project_id: String,
    agent_id: String,
    _repo_dir: TestDir,
    _workspaces_root: TestDir,
}

async fn setup_remote_roundtrip(prefix: &str) -> RemoteRoundtripFixture {
    let repo_dir = TestDir::new(&format!("{prefix}-repo"));
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces_root = TestDir::new(&format!("{prefix}-workspaces"));
    let harness = test_app(workspaces_root.path(), prefix).await;

    let registration = register_daemon(&harness.app, &format!("{prefix}-machine"), prefix).await;
    let server = TestServer::start(Arc::clone(&harness.state)).await;
    let daemon_socket = connect_daemon(
        &server,
        &registration.daemon_id,
        Some(&registration.registration_token),
    )
    .await
    .expect("daemon websocket upgrade succeeds");
    wait_until_connected(&harness.state, &registration.daemon_id).await;
    report_remote_daemon_shell(
        &harness.app,
        &registration.daemon_id,
        &registration.registration_token,
        workspaces_root.path(),
        prefix,
    )
    .await;

    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, &format!("{prefix} Project"), &repo_path).await;

    let agent: AgentResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        &admin_jwt(),
        json!({
            "name": format!("{prefix}-shell-agent"),
            "executor_type": "shell",
            "daemon_id": registration.daemon_id,
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(agent.effective_status.as_deref(), Some("active"));

    RemoteRoundtripFixture {
        harness,
        registration,
        server,
        daemon_socket,
        project_id,
        agent_id: agent.id,
        _repo_dir: repo_dir,
        _workspaces_root: workspaces_root,
    }
}

async fn start_execution_with_accepted_daemon(
    harness: &common::Harness,
    daemon_socket: &mut common::fake_daemon::ClientSocket,
    project_id: &str,
    agent_id: &str,
    title: &str,
    description: &str,
) -> (String, String) {
    let created_task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({
            "title": title,
            "description": description,
        }),
        StatusCode::OK,
    )
    .await;
    let task_id = created_task.id.clone();
    let (execution_id, _) =
        launch_execution_for_task(harness, daemon_socket, &task_id, agent_id, description).await;
    (task_id, execution_id)
}

async fn launch_execution_for_task(
    harness: &common::Harness,
    daemon_socket: &mut common::fake_daemon::ClientSocket,
    task_id: &str,
    agent_id: &str,
    prompt: &str,
) -> (String, Value) {
    harness
        .state
        .task_service
        .create_task_role(
            &task_id,
            "implementer",
            db::CoordinationMode::Independent,
            "{}".to_owned(),
        )
        .await
        .expect("implementer TaskRole creates");
    harness
        .state
        .task_service
        .add_task_role_member(
            &task_id,
            "implementer",
            api_types::ActorRef::Agent(agent_id.to_owned()),
        )
        .await
        .expect("Agent joins implementer TaskRole");

    let start_app = harness.app.clone();
    let start_task_id = task_id.to_owned();
    let start_agent_id = agent_id.to_owned();
    let prompt = prompt.to_owned();
    let start_handle = tokio::spawn(async move {
        json_request::<ExecutionResponse>(
            &start_app,
            Method::POST,
            &format!("/api/v1/tasks/{start_task_id}/executions"),
            json!({
                "agent_id": start_agent_id,
                "role": "implementer",
                "purpose": "implement",
                "prompt": prompt,
                "input_artifact_ids": []
            }),
            StatusCode::OK,
        )
        .await
    });

    let (start_id, start_params) = next_daemon_request(daemon_socket, METHOD_EXECUTION_START).await;
    let execution_id = start_params["execution_id"]
        .as_str()
        .expect("execution id in start params")
        .to_owned();
    send_daemon_response(
        daemon_socket,
        start_id,
        api_types::ExecutionStartResult {
            execution_id: execution_id.clone(),
            accepted: true,
        },
    )
    .await;

    let execution = start_handle.await.expect("Execution start joins");
    assert_eq!(execution.id, execution_id);
    assert_eq!(execution.role, "implementer");
    assert_eq!(
        execution.purpose,
        Some(api_types::ExecutionPurpose::Implement)
    );
    assert_eq!(
        execution.actor_ref,
        Some(api_types::ActorRef::Agent(agent_id.to_owned()))
    );

    (execution_id, start_params)
}

#[tokio::test]
async fn remote_execution_completes_and_transitions_task() {
    let mut fixture = setup_remote_roundtrip("remote-roundtrip-success").await;

    let (task_id, execution_id) = start_execution_with_accepted_daemon(
        &fixture.harness,
        &mut fixture.daemon_socket,
        &fixture.project_id,
        &fixture.agent_id,
        "Remote roundtrip success",
        "echo remote success",
    )
    .await;

    send_execution_log(
        &mut fixture.daemon_socket,
        &execution_id,
        1,
        "remote line one",
    )
    .await;
    send_execution_log(
        &mut fixture.daemon_socket,
        &execution_id,
        2,
        "remote line two",
    )
    .await;
    send_execution_log(
        &mut fixture.daemon_socket,
        &execution_id,
        3,
        "remote line three",
    )
    .await;
    send_execution_terminal_completed(
        &mut fixture.daemon_socket,
        &execution_id,
        Some("remote shell finished"),
    )
    .await;

    let completed = poll_until_execution_status(
        &fixture.harness.state,
        &execution_id,
        DbExecutionStatus::Completed,
    )
    .await;
    assert_eq!(completed.status, DbExecutionStatus::Completed);

    let lifecycle = poll_task_lifecycle(
        &fixture.harness.state.db,
        &task_id,
        TaskLifecycleState::Active,
    )
    .await;
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);

    let logs = fetch_execution_logs(&fixture.harness.app, &execution_id).await;
    let entries = logs["items"].as_array().expect("log items array");
    assert!(
        entries.len() >= 3,
        "expected persisted execution logs, got {}",
        entries.len()
    );
    let lines: Vec<&str> = entries
        .iter()
        .filter_map(|entry| {
            entry
                .get("payload")
                .and_then(|payload| payload.get("line"))
                .and_then(Value::as_str)
        })
        .collect();
    assert!(lines.iter().any(|line| line.contains("remote line one")));
    assert!(lines.iter().any(|line| line.contains("remote line three")));
}

#[tokio::test]
async fn remote_execution_failure_does_not_mutate_aggregate_task_lifecycle() {
    let mut fixture = setup_remote_roundtrip("remote-roundtrip-failure").await;

    let (task_id, execution_id) = start_execution_with_accepted_daemon(
        &fixture.harness,
        &mut fixture.daemon_socket,
        &fixture.project_id,
        &fixture.agent_id,
        "Remote roundtrip failure",
        "this should fail remotely",
    )
    .await;

    send_execution_terminal_failed(
        &mut fixture.daemon_socket,
        &execution_id,
        "remote executor exploded",
    )
    .await;

    let failed = wait_for_execution_status(
        &fixture.harness.state,
        &execution_id,
        DbExecutionStatus::Failed,
    )
    .await;
    assert_eq!(failed.stop_reason, Some(StopReason::ExecutorFailed));
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|message| message.contains("remote executor exploded")),
        "unexpected execution error: {:?}",
        failed.error
    );

    // Execution outcome is historical work evidence. A failed Execution does
    // not itself rewrite aggregate TaskLifecycle; exact retry receipts and
    // Gate facts own any later lifecycle effects.
    let lifecycle = poll_task_lifecycle(
        &fixture.harness.state.db,
        &task_id,
        TaskLifecycleState::Active,
    )
    .await;
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);
}

#[tokio::test]
async fn remote_daemon_disconnect_does_not_mutate_aggregate_task_lifecycle() {
    let mut fixture = setup_remote_roundtrip("remote-roundtrip-disconnect").await;

    let (task_id, execution_id) = start_execution_with_accepted_daemon(
        &fixture.harness,
        &mut fixture.daemon_socket,
        &fixture.project_id,
        &fixture.agent_id,
        "Remote disconnect",
        "running until disconnect",
    )
    .await;

    fixture
        .daemon_socket
        .send(WsMessage::Close(None))
        .await
        .expect("close daemon websocket");
    drop(fixture.server);
    wait_until_disconnected(&fixture.harness.state, &fixture.registration.daemon_id).await;

    let monitor = HeartbeatMonitor::new(
        Arc::clone(&fixture.harness.state.db),
        Arc::clone(&fixture.harness.state.event_bus),
    )
    .with_task_service(fixture.harness.state.task_service.clone())
    .with_daemon_connections(fixture.harness.state.daemon_connections.clone())
    .with_daemon_disconnect_grace(Duration::ZERO);

    let interrupted = monitor.check_once().await.expect("heartbeat monitor runs");
    assert_eq!(interrupted, 1);

    let execution = ExecutionRepo::get_by_id(&*fixture.harness.state.db, &execution_id)
        .await
        .expect("execution loads")
        .expect("execution exists");
    assert_eq!(execution.status, DbExecutionStatus::Failed);
    assert_eq!(execution.stop_reason, Some(StopReason::DaemonDisconnected));

    let lifecycle = poll_task_lifecycle(
        &fixture.harness.state.db,
        &task_id,
        TaskLifecycleState::Active,
    )
    .await;
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);
}

/// Fallback-chain round-trip over the daemon protocol: the snapshot carries
/// the route to the daemon, and the structured terminal notification carries
/// disposition, attempts, and the winner back for persistence.
#[tokio::test]
async fn remote_executor_unavailability_preserves_route_without_legacy_task_projection() {
    let mut fixture = setup_remote_roundtrip("remote-unavailable").await;

    // A routed legacy shell agent: fallback may vary its command while
    // preserving the Agent's stable harness identity.
    let routed_agent: AgentResponse = json_request_with_bearer(
        &fixture.harness.app,
        Method::POST,
        "/api/v1/agents",
        &admin_jwt(),
        json!({
            "name": "remote-unavailable-routed-agent",
            "executor_type": "shell",
            "daemon_id": fixture.registration.daemon_id,
            "config_json": {
                "fallbacks": [ { "executor_type": "shell", "config": {"command":"echo fallback"} } ]
            },
        }),
        StatusCode::OK,
    )
    .await;

    let created_task: TaskResponse = json_request(
        &fixture.harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", fixture.project_id),
        json!({
            "title": "Remote unavailable roundtrip",
            "description": "exhaust every candidate",
        }),
        StatusCode::OK,
    )
    .await;
    let task_id = created_task.id.clone();
    let (execution_id, start_params) = launch_execution_for_task(
        &fixture.harness,
        &mut fixture.daemon_socket,
        &task_id,
        &routed_agent.id,
        "exhaust every candidate",
    )
    .await;
    // Server → daemon: the snapshot carries the full route.
    let routing = &start_params["executor_config"]["routing"];
    assert_eq!(routing["policy"], "ordered_fallback_v1");
    assert_eq!(
        routing["candidates"].as_array().expect("candidates").len(),
        2
    );

    // Daemon → server: every candidate exhausted, retry known in ~90s.
    let retry_at = (chrono::Utc::now() + chrono::Duration::seconds(90)).to_rfc3339();
    common::fake_daemon::send_daemon_notification(
        &mut fixture.daemon_socket,
        api_types::METHOD_EXECUTION_TERMINAL,
        api_types::ExecutionTerminalNotification {
            execution_id: execution_id.clone(),
            exit_code: Some(1),
            signal: None,
            error: Some("no executor candidate available".to_owned()),
            ts: db::now_rfc3339(),
            status: Some("failed".to_owned()),
            agent_session_id: None,
            summary: None,
            assistant_output: None,
            after_sha: None,
            usage: None,
            account_usage: None,
            failure_class: Some(api_types::RemoteExecutionFailureClass::ExecutorUnavailable),
            retry_at: Some(retry_at),
            resolved_candidate: None,
            route_attempts: Some(vec![
                api_types::RemoteRouteAttempt {
                    candidate_key: "shell#primary".to_owned(),
                    outcome: "usage_exhausted".to_owned(),
                },
                api_types::RemoteRouteAttempt {
                    candidate_key: "shell#fallback".to_owned(),
                    outcome: "unavailable".to_owned(),
                },
            ]),
        },
    )
    .await;

    let failed = poll_until_execution_status(
        &fixture.harness.state,
        &execution_id,
        DbExecutionStatus::Failed,
    )
    .await;
    assert_eq!(failed.status, DbExecutionStatus::Failed);

    // TaskLifecycle and old retry metadata do not derive from an unavailable
    // harness route. The exact route disposition is retained on this
    // Execution for an explicit later decision.
    let task = TaskRepo::get_by_id(&*fixture.harness.state.db, &task_id, false)
        .await
        .expect("task lookup")
        .expect("task exists");
    let metadata: Value = task
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| json!({}));
    assert!(metadata.get("execution_retry_count").is_none());
    assert!(metadata.get("deferred_dispatch").is_none());
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.harness.state.db, &task_id)
        .await
        .expect("TaskLifecycle loads")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);

    // Attempts and disposition are persisted on the execution snapshot.
    let stored = ExecutionRepo::get_by_id(&*fixture.harness.state.db, &execution_id)
        .await
        .expect("execution lookup")
        .expect("execution exists");
    let snapshot: Value = serde_json::from_str(
        stored
            .executor_config_snapshot_json
            .as_deref()
            .expect("snapshot present"),
    )
    .expect("snapshot parses");
    assert_eq!(
        snapshot["routing"]["attempts"][0]["outcome"],
        "usage_exhausted"
    );
    assert_eq!(
        snapshot["routing"]["disposition"]["failure_class"],
        "executor_unavailable"
    );
}
